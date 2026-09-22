// the desktop binary, everything else is in lib.rs (android loads that as a .so and never comes through here)

// a gui app, not a console one: without this every launch on windows drags a black console window along
#![cfg_attr(windows, windows_subsystem = "windows")]

// a gui subsystem exe starts without a console, so --help and friends would print into the void when run from
// a terminal. hooking onto the parent's console (if it has one, a double click in explorer has none) brings that back
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn AttachConsole(process_id: u32) -> i32;
}

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    unsafe {
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
    tacoshell::main()
}
