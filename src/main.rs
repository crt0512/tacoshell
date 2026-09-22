// the desktop binary, everything else is in lib.rs (android loads that as a .so and never comes through here)

fn main() -> eframe::Result<()> {
    tacoshell::main()
}
