// tacoshell, a taco wrap for tui apps
//
// gives a terminal app its own window, dock icon and all. either a program on this machine 
// or a session on a server over ssh with a login screen and reconnect handling. what it wraps and everything it shows
// comes from the config baked in at build time, see tacoshell.toml

mod config;
mod creds;
mod schema;
mod session;
mod state;
mod term;
mod ui;

use eframe::egui;

const HELP: &str = "\
options:
  --kiosk              kiosk mode: fullscreen, status bar, on screen keyboard
  --meta               print what the package gets called and described as
  --write-icon FILE    write the baked in icon png to FILE (fails if there is none)
  --print-config       print the baked in config
  --version
  --help";

fn main() -> eframe::Result<()> {
    let cfg = config::load();

    let mut kiosk = cfg.kiosk.enabled;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--kiosk" => kiosk = true,
            "--meta" => {
                // same as the tacoshell-meta.env build.rs puts next to the binary
                print!("{}", config::meta());
                return Ok(());
            }
            "--write-icon" => {
                let (Some(path), Some(png)) = (args.next(), config::icon_png()) else {
                    eprintln!("no icon baked in (or no FILE given)");
                    std::process::exit(1);
                };
                if let Err(e) = std::fs::write(&path, png) {
                    eprintln!("{path}: {e}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            "--print-config" => {
                print!("{}", config::baked_toml());
                return Ok(());
            }
            "--version" => {
                let tacoshell = env!("CARGO_PKG_VERSION");
                match &cfg.app.version {
                    Some(v) => println!("{} {v} (tacoshell {tacoshell})", cfg.app.name),
                    None => println!("{} (tacoshell {tacoshell})", cfg.app.name),
                }
                return Ok(());
            }
            "--help" | "-h" => {
                println!("{}\n\n{HELP}", cfg.app.name);
                return Ok(());
            }
            other => {
                eprintln!("unknown option {other}\n\n{HELP}");
                std::process::exit(2);
            }
        }
    }

    // ssh lives on here, the ui thread only ever pokes it through channels
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("cant start the async runtime");

    // guess the window size from the cell count, the real cell size is only known after the first frame
    let t = &cfg.terminal;
    let est_width = t.cols as f32 * t.font_size * 0.61 + 20.0;
    let est_height = t.rows as f32 * t.font_size * 1.29 + 20.0;

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(&cfg.app.name)
        .with_app_id(&cfg.app.id)
        .with_inner_size([est_width, est_height])
        .with_min_inner_size([300.0, 200.0]);
    if let Some(icon) = config::icon_png().and_then(load_icon) {
        viewport = viewport.with_icon(icon);
    }
    if kiosk && cfg.kiosk.fullscreen {
        viewport = viewport.with_fullscreen(true);
    }

    let name = cfg.app.name.clone();
    let handle = rt.handle().clone();
    eframe::run_native(
        &name,
        eframe::NativeOptions { viewport, ..Default::default() },
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            cc.egui_ctx.set_fonts(fonts());
            Ok(Box::new(ui::TacoApp::new(cc, cfg, handle, kiosk)))
        }),
    )
}

fn load_icon(png: &[u8]) -> Option<egui::IconData> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

// goofy ah font fallback chain : "Hack"sor Font -> NotoSansMono -> NotoSansSymbols2 (Undertale)
// But for some reason it doesnt work right in this app so I got autistic and did the following :
// NotoSansMono looks somewhat decent but no symbols (also called dingbats afaik) ... which i need.
// NotoSansSymbols2 (without-subset) has the symbols but is way too fat. With -subset is me taking out most crap I dont use and slimin it down.
fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "NotoSansMono".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansMono-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "NotoSansSymbols2".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/NotoSansSymbols2-subset.ttf"
        ))),
    );
    for family in [egui::FontFamily::Monospace, egui::FontFamily::Proportional] {
        let list = fonts.families.entry(family).or_default();
        list.push("NotoSansMono".to_owned());
        list.push("NotoSansSymbols2".to_owned());
    }
    fonts
}
