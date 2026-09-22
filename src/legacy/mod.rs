// android 4.0.3 and up (make apk-legacy). eframe draws through wgpu, which wants vulkan or opengl es 3, or through
// glow + glutin, which looks every gl function up with eglGetProcAddress. android 4 tablets have opengl es 2 and an
// eglGetProcAddress that cant do that (see egl.rs), so the legacy apk runs the app itself: winit for the window and
// input like eframe does, egl by hand, egui_glow to draw. it drives the same eframe::App as always, set up with the
// constructors eframe has for running apps without its own window (_new_kittest, for tests really)

mod egl;
mod libc_shims;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, ViewportCommand, ViewportId};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId};

/// egui wants another frame. comes from whatever thread asked, ssh output arrives on tokio's
struct Repaint {
    delay: Duration,
    /// passes done when it was asked. if one ran since, it already knew
    pass: u64,
}

/// eframe::run_native for android 4
pub fn run_native(options: eframe::NativeOptions, creator: eframe::AppCreator<'_>) -> eframe::Result<()> {
    let android_app = options.android_app.expect("android_main hands over the AndroidApp");
    let event_loop = EventLoop::<Repaint>::with_user_event().with_android_app(android_app).build()?;

    let ctx = egui::Context::default();
    let proxy = Mutex::new(event_loop.create_proxy());
    ctx.set_request_repaint_callback(move |info| {
        if let Ok(proxy) = proxy.lock() {
            let _ = proxy.send_event(Repaint { delay: info.delay, pass: info.current_cumulative_pass_nr });
        }
    });
    let app = creator(&eframe::CreationContext::_new_kittest(ctx.clone())).map_err(eframe::Error::AppCreation)?;

    let mut runner = Runner {
        ctx,
        app,
        frame: eframe::Frame::_new_kittest(),
        egl: None,
        painter: None,
        input: None,
        window: None,
        next_frame: Some(Instant::now()),
    };
    event_loop.run_app(&mut runner)?;
    runner.app.on_exit();
    Ok(())
}

struct Runner<'app> {
    ctx: egui::Context,
    app: Box<dyn eframe::App + 'app>,
    frame: eframe::Frame,
    /// the gl context, painter and input state stay for the whole run
    egl: Option<egl::Egl>,
    painter: Option<egui_glow::Painter>,
    input: Option<egui_winit::State>,
    /// only while the app is in the foreground, android takes it away otherwise
    window: Option<(Window, egl::Surface)>,
    next_frame: Option<Instant>,
}

impl Runner<'_> {
    fn attach(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let window = event_loop.create_window(Window::default_attributes()).map_err(|e| e.to_string())?;
        let native = match window.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::AndroidNdk(h) => h.a_native_window.as_ptr(),
            other => return Err(format!("not an android window: {other:?}")),
        };
        if self.egl.is_none() {
            self.egl = Some(egl::Egl::new()?);
        }
        let egl = self.egl.as_ref().unwrap();
        let surface = egl.attach(native)?;
        if self.painter.is_none() {
            let painter =
                egui_glow::Painter::new(Arc::new(egl.gl()?), "", None, true).map_err(|e| format!("egui_glow: {e}"))?;
            let ppp = Some(window.scale_factor() as f32);
            self.input = Some(egui_winit::State::new(
                self.ctx.clone(),
                ViewportId::ROOT,
                event_loop,
                ppp,
                None,
                Some(painter.max_texture_side()),
            ));
            self.painter = Some(painter);
        }
        window.request_redraw();
        self.window = Some((window, surface));
        Ok(())
    }

    fn paint(&mut self, event_loop: &ActiveEventLoop) {
        let (Some((window, surface)), Some(input), Some(painter), Some(egl)) =
            (&self.window, &mut self.input, &mut self.painter, &self.egl)
        else {
            return;
        };
        let (app, frame) = (&mut self.app, &mut self.frame);
        let mut output = self.ctx.run_ui(input.take_egui_input(window), |ui| {
            app.logic(ui.ctx(), frame);
            app.ui(ui, frame);
        });
        input.handle_platform_output(window, std::mem::take(&mut output.platform_output));

        let primitives = self.ctx.tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let size = window.inner_size();
        let size = [size.width, size.height];
        painter.clear(size, app.clear_color(&self.ctx.global_style().visuals));
        painter.paint_and_update_textures(size, output.pixels_per_point, &primitives, &mut output.textures_delta);
        egl.swap(*surface);

        if let Some(root) = output.viewport_output.get(&ViewportId::ROOT) {
            self.handle(event_loop, &root.commands);
            self.frame_in(root.repaint_delay);
        }
    }

    /// in the background theres no window to draw in, but ssh output still wants taking in and timers still run
    fn logic_only(&mut self, event_loop: &ActiveEventLoop) {
        let (app, frame) = (&mut self.app, &mut self.frame);
        let output = self.ctx.run_logic(&egui::RawInput::default(), |ctx| app.logic(ctx, frame));
        if let Some(commands) = output.viewport_commands.get(&ViewportId::ROOT) {
            self.handle(event_loop, commands);
        }
    }

    fn handle(&mut self, event_loop: &ActiveEventLoop, commands: &[ViewportCommand]) {
        // the rest (title, size, fullscreen...) means nothing on a phone
        if commands.contains(&ViewportCommand::Close) {
            event_loop.exit();
        }
    }

    fn frame_in(&mut self, delay: Duration) {
        // Duration::MAX is egui for "no need"
        if let Some(at) = Instant::now().checked_add(delay) {
            self.next_frame = Some(self.next_frame.map_or(at, |t| t.min(at)));
        }
    }
}

impl ApplicationHandler<Repaint> for Runner<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(e) = self.attach(event_loop) {
            log::error!("cant draw: {e}");
            event_loop.exit();
        }
    }

    fn suspended(&mut self, _: &ActiveEventLoop) {
        // the surface has to go before android destroys the window under it
        if let Some((window, surface)) = self.window.take() {
            if let Some(egl) = &self.egl {
                egl.detach(surface);
            }
            drop(window);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let (Some((window, _)), Some(input)) = (&self.window, &mut self.input) else { return };
        let repaint = input.on_window_event(window, &event).repaint;
        match event {
            WindowEvent::RedrawRequested => self.paint(event_loop),
            WindowEvent::CloseRequested => event_loop.exit(),
            _ if repaint => window.request_redraw(),
            _ => {}
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, repaint: Repaint) {
        if repaint.pass >= self.ctx.cumulative_pass_nr() {
            self.frame_in(repaint.delay);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.next_frame.is_some_and(|at| at <= Instant::now()) {
            self.next_frame = None;
            match &self.window {
                Some((window, _)) => window.request_redraw(),
                None => self.logic_only(event_loop),
            }
        }
        event_loop.set_control_flow(match self.next_frame {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        // gl resources only go with the context current, which it is while theres a window
        if let (Some(painter), Some(_)) = (&mut self.painter, &self.window) {
            painter.destroy();
        }
    }
}
