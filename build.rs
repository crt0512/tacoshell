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
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let mac_icon = base.join("logo.icns");
    let has_icon = if target_os == "macos" && mac_icon.is_file() {
        println!("cargo:rerun-if-changed={}", mac_icon.display());
        let result = std::process::Command::new("sips")
            .args(["-s", "format", "png"])
            .arg(&mac_icon)
            .args(["--out"])
            .arg(&icon_out)
            .output()
            .unwrap_or_else(|e| fail(&mac_icon, &format!("cant run sips: {e}")));
        if !result.status.success() {
            fail(
                &mac_icon,
                &format!(
                    "sips failed: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                ),
            );
        }
        true
    } else {
        match cfg.app.icon.take() {
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
        }
    };

    if let Err(e) = cfg.validate() {
        fail(&path, &e);
    }
    // what building for a phone needs on top
    if matches!(target_os.as_str(), "android" | "ios") && cfg.local.is_some() {
        fail(&path, "[local] cant run on phones and tablets (no ptys there), build this one from an [ssh] config");
    }
    if target_os == "android"
        && let Err(e) = cfg.android_package()
    {
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
    // the exe's icon and version info, what explorer and task manager show
    if target_os == "windows" {
        windows_resources(&cfg, &out_dir, has_icon.then_some(icon_out.as_path()));
    }

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
        // empty when app.id doesnt work for android, an android build already failed over that
        ("ANDROID_PACKAGE", cfg.android_package().unwrap_or_default()),
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

/// what explorer shows for the exe: its icon and the version tab. an .rc compiled to an object that gets linked into the
/// bin (not the .so, not the tests). zig rc does that (a cross build from linux has zig anyway), mingw's windres or
/// microsoft's rc.exe otherwise. with none of them the exe still works, it just has no icon, so thats a warning not an error
fn windows_resources(cfg: &schema::Config, out_dir: &Path, icon_png: Option<&Path>) {
    // "" is a quote inside an rc string, backslashes are escapes
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\"\""));
    let version = cfg.app.version.clone().unwrap_or_else(|| env::var("CARGO_PKG_VERSION").unwrap());
    // FILEVERSION wants four numbers, whatever digits the version has go there ("1.2.3-beta" -> 1,2,3,0)
    let mut nums = [0u16; 4];
    for (n, part) in nums.iter_mut().zip(version.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty())) {
        *n = part.parse().unwrap_or(0);
    }
    let nums = format!("{},{},{},{}", nums[0], nums[1], nums[2], nums[3]);

    let mut rc = String::new();
    if let Some(png) = icon_png {
        // an .ico is a little directory in front of the images, and since vista an image may just be a png as is.
        // 6 byte header, one 16 byte entry, then the png. width and height are single bytes, 0 means 256 (or more)
        let bytes = fs::read(png).unwrap();
        let dim = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        let side = |d: u32| if d >= 256 { 0u8 } else { d as u8 };
        let mut ico = Vec::with_capacity(22 + bytes.len());
        ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
        ico.extend_from_slice(&[side(dim(16)), side(dim(20)), 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(&bytes);
        fs::write(out_dir.join("icon.ico"), ico).unwrap();
        // the lowest id is the one explorer picks for the file
        rc.push_str("1 ICON \"icon.ico\"\n");
    }
    let description = if cfg.app.description.is_empty() { &cfg.app.name } else { &cfg.app.description };
    // FILEOS 0x40004 = VOS_NT_WINDOWS32, FILETYPE 1 = VFT_APP, 0x409/1200 = english, unicode. the block name has to match
    rc.push_str(&format!(
        "1 VERSIONINFO\nFILEVERSION {nums}\nPRODUCTVERSION {nums}\nFILEOS 0x40004\nFILETYPE 1\nBEGIN\n\
         BLOCK \"StringFileInfo\"\nBEGIN\nBLOCK \"040904B0\"\nBEGIN\n\
         VALUE \"ProductName\", {name}\nVALUE \"FileDescription\", {desc}\n\
         VALUE \"ProductVersion\", {ver}\nVALUE \"FileVersion\", {ver}\n\
         VALUE \"InternalName\", {bin}\nVALUE \"OriginalFilename\", {exe}\n\
         END\nEND\nBLOCK \"VarFileInfo\"\nBEGIN\nVALUE \"Translation\", 0x409, 1200\nEND\nEND\n",
        name = quote(&cfg.app.name),
        desc = quote(description),
        ver = quote(&version),
        bin = quote(&cfg.app.binary),
        exe = quote(&format!("{}.exe", cfg.app.binary)),
    ));
    fs::write(out_dir.join("app.rc"), rc).unwrap();

    // whichever resource compiler is around. all run in OUT_DIR so "icon.ico" in the rc resolves, and everything gets
    // the target from the triple so a cross build doesnt end up with an object for the build machine
    let target = env::var("TARGET").unwrap();
    // there or not is all that matters, rc.exe has no --version to ask. a broken one fails below, with its stderr
    let have = |tool: &str| std::process::Command::new(tool).arg("--version").output().is_ok();
    // mingw calls 32 bit i686 where rust says x86
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let windres = format!("{}-w64-mingw32-windres", if arch == "x86" { "i686" } else { &arch });
    let (mut cmd, object) = if have("zig") {
        let mut c = std::process::Command::new("zig");
        c.args(["rc", "/:output-format", "coff", "/:target", &target, "/c", "65001", "/fo", "app.o", "app.rc"]);
        (c, "app.o")
    } else if let Some(windres) = [windres.as_str(), "windres"].into_iter().find(|t| have(t)) {
        let mut c = std::process::Command::new(windres);
        c.args(["-c", "65001", "-O", "coff", "-o", "app.o", "app.rc"]);
        (c, "app.o")
    } else if have("rc") {
        // microsoft's, makes a .res and link.exe takes that as it is
        let mut c = std::process::Command::new("rc");
        c.args(["/nologo", "/c", "65001", "/fo", "app.res", "app.rc"]);
        (c, "app.res")
    } else {
        println!("cargo:warning=no resource compiler (zig, {windres} or rc.exe), the exe gets no icon and no version info");
        return;
    };
    match cmd.current_dir(out_dir).output() {
        Ok(o) if o.status.success() => {
            println!("cargo:rustc-link-arg-bins={}", out_dir.join(object).display());
        }
        Ok(o) => println!(
            "cargo:warning=resource compiler failed, the exe gets no icon and no version info: {}",
            String::from_utf8_lossy(&o.stderr).trim().replace('\n', " ")
        ),
        Err(e) => println!("cargo:warning=cant run the resource compiler, the exe gets no icon and no version info: {e}"),
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
