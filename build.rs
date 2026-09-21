// bakes the central config into the binary
//
// reads TACOSHELL_CONFIG (default tacoshell.toml next to this file), checks it,
// pulls in the files it points at (logo, icon) and writes the lot to OUT_DIR where src/main.rs includes it. a bad config fails right here in the build

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "src/schema.rs"]
#[allow(dead_code)]
mod schema;

fn main() {
    println!("cargo:rerun-if-env-changed=TACOSHELL_CONFIG");
    println!("cargo:rerun-if-changed=src/schema.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let path = env::var_os("TACOSHELL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tacoshell.toml"));
    let path = if path.is_absolute() { path } else { manifest_dir.join(path) };
    println!("cargo:rerun-if-changed={}", path.display());
    let base = path.parent().unwrap_or(Path::new("."));

    let src = fs::read_to_string(&path)
        .unwrap_or_else(|e| fail(&path, &format!("cant read it: {e}")));
    let mut cfg: schema::Config = toml::from_str(&src).unwrap_or_else(|e| fail(&path, &e.to_string()));

    if let Some(file) = cfg.login.logo_file.take() {
        let file = base.join(file);
        println!("cargo:rerun-if-changed={}", file.display());
        cfg.login.logo = fs::read_to_string(&file)
            .unwrap_or_else(|e| fail(&path, &format!("login.logo_file {}: {e}", file.display())));
    }

    let icon_out = out_dir.join("icon.png");
    let mut icon_size = String::new();
    let has_icon = match cfg.app.icon.take() {
        Some(file) => {
            let file = base.join(file);
            println!("cargo:rerun-if-changed={}", file.display());
            let bytes = fs::read(&file)
                .unwrap_or_else(|e| fail(&path, &format!("app.icon {}: {e}", file.display())));
            if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.len() < 24 {
                fail(&path, &format!("app.icon {} isnt a png", file.display()));
            }
            // width and height sit right in the IHDR chunk, the desktop icon dir is named after them
            let dim = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
            icon_size = format!("{}x{}", dim(16), dim(20));
            fs::write(&icon_out, bytes).unwrap();
            true
        }
        None => false,
    };

    if let Err(e) = cfg.validate() {
        fail(&path, &e);
    }

    if cfg.app.binary == schema::DEFAULT_BINARY {
        println!(
            "cargo:warning=Please change the binary option in your tacoshell.toml to something that \
             fits your usecase and not the default tacoshell name!"
        );
    }
    if cfg.ssh.as_ref().is_some_and(|s| s.credentials == schema::Credentials::Unsafe) {
        println!(
            "cargo:warning=ssh.credentials = \"unsafe\": passwords get saved base64 encoded, not encrypted, DO NOT FUCKING SHIP THIS UNLESS YOU KNOW WHAT YOU'RE DOING"
        );
    }

    fs::write(out_dir.join("config.toml"), toml::to_string(&cfg).unwrap()).unwrap();
    write_meta(&cfg, &out_dir, has_icon.then_some(icon_size.as_str()));

    let icon = if has_icon {
        "Some(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/icon.png\")))"
    } else {
        "None"
    };
    fs::write(
        out_dir.join("baked.rs"),
        format!(
            "pub const CONFIG: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/config.toml\"));\n\
             pub const META: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/meta.env\"));\n\
             pub const ICON: Option<&[u8]> = {icon};\n"
        ),
    )
    .unwrap();
}

/// KEY=value lines for the makefile, so packaging never has to run the binary (which it couldnt anyway when it was built for another architecture). 
/// lands in OUT_DIR and right next to the binary as tacoshell-meta.env, the icon as tacoshell-icon.png
fn write_meta(cfg: &schema::Config, out_dir: &Path, icon_size: Option<&str>) {
    let one_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let p = &cfg.packaging;
    let lines = [
        ("NAME", cfg.app.name.clone()),
        ("ID", cfg.app.id.clone()),
        ("BINARY", cfg.app.binary.clone()),
        ("VERSION", cfg.app.version.clone().unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap())),
        // a package without a description isnt one, the name is better than nothing
        ("DESCRIPTION", one_line(if cfg.app.description.is_empty() { &cfg.app.name } else { &cfg.app.description })),
        ("LONG_DESCRIPTION", one_line(&p.long_description.clone().unwrap_or_else(|| default_long_description(cfg)))),
        ("MAINTAINER", p.maintainer.clone().unwrap_or_default()),
        ("LICENSE", p.license.clone().unwrap_or_default()),
        ("HOMEPAGE", p.homepage.clone().unwrap_or_default()),
        ("DEPENDS", cfg.local.as_ref().map(|l| l.depends.join(", ")).unwrap_or_default()),
        ("ICON_SIZE", icon_size.unwrap_or_default().to_string()),
    ];
    let text: String = lines.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    fs::write(out_dir.join("meta.env"), &text).unwrap();

    // OUT_DIR is <target>/[<triple>/]<profile>/build/tacoshell-<hash>/out, three up is where the binary goes
    if let Some(profile_dir) = out_dir.ancestors().nth(3) {
        let _ = fs::write(profile_dir.join("tacoshell-meta.env"), &text);
        let icon = profile_dir.join("tacoshell-icon.png");
        let _ = match icon_size {
            Some(_) => fs::copy(out_dir.join("icon.png"), &icon).map(|_| ()),
            None => fs::remove_file(&icon).or(Ok(())),
        };
    }
}

/// something that says more than the one line description (debian wants them to differ)
fn default_long_description(cfg: &schema::Config) -> String {
    match &cfg.local {
        Some(local) => format!(
            "{} in a window of its own: it runs {} in a terminal built into the window.",
            cfg.app.name, local.program
        ),
        None => format!(
            "{} in a window of its own: it logs into a server over SSH and runs the app there, \
             reconnecting by itself when the connection drops.",
            cfg.app.name
        ),
    }
}

fn fail(path: &Path, msg: &str) -> ! {
    panic!("\n\ntacoshell config {}:\n{msg}\n\n", path.display());
}
