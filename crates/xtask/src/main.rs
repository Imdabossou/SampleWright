//! Release packaging: `cargo xtask package [options]`.
//!
//! Builds both programs (samplewright, the app, and samplewright-cli, the command line tool)
//! and packages them with the README, the licence, the licences of the works
//! SampleWright derives from (LICENSES/) and THIRD_PARTY_LICENSES.txt, collected
//! from every linked crate's own licence files:
//!
//!   Linux    dist/samplewright-<version>-linux-<arch>.tar.gz (and .AppImage with
//!            --appimage, which needs appimagetool on PATH or in $APPIMAGETOOL)
//!   Windows  dist/samplewright-<version>-windows-<arch>.zip
//!   macOS    dist/samplewright-<version>-macos-<arch>.zip holding SampleWright.app
//!            (its icon needs iconutil, i.e. a Mac)
//!
//! Options: --target <rust triple>, --platform <name in the file name>,
//! --with-models (bundle models/ for offline use), --appimage.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

const LICENSE_PREFIXES: [&str; 5] = ["license", "licence", "copying", "notice", "copyright"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|_| PathBuf::from("."))
}

fn run(cmd: &mut Command) -> Result<String> {
    eprintln!("+ {cmd:?}");
    let out = cmd.output().with_context(|| format!("running {cmd:?}"))?;
    if !out.status.success() {
        bail!("{cmd:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn cargo() -> Command {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.current_dir(root());
    c
}

fn host_triple() -> Result<String> {
    let v = run(Command::new("rustc").arg("-vV"))?;
    v.lines().find_map(|l| l.strip_prefix("host: ")).map(str::to_string).context("no host triple from rustc -vV")
}

fn platform_of(triple: &str) -> String {
    let os = if triple.contains("windows") {
        "windows"
    } else if triple.contains("apple") {
        "macos"
    } else {
        "linux"
    };
    let arch = if triple.starts_with("aarch64") { "arm64" } else { "x64" };
    format!("{os}-{arch}")
}

fn metadata(args: &[&str]) -> Result<Value> {
    Ok(serde_json::from_str(&run(cargo().args(["metadata", "--format-version", "1"]).args(args))?)?)
}

fn license_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.file_name().is_some_and(|n| LICENSE_PREFIXES.iter().any(|pre| n.to_string_lossy().to_lowercase().starts_with(pre))))
        .collect();
    v.sort();
    v
}

/// Licence texts of every crate linked into the two programs on `triple`,
/// plus the works SampleWright derives from.
fn third_party_licenses(triple: &str) -> Result<String> {
    let meta = metadata(&["--locked", "--filter-platform", triple])?;
    let pkgs: BTreeMap<&str, &Value> = meta["packages"].as_array().into_iter().flatten().map(|p| (p["id"].as_str().unwrap_or(""), p)).collect();
    let nodes: BTreeMap<&str, &Value> =
        meta["resolve"]["nodes"].as_array().into_iter().flatten().map(|n| (n["id"].as_str().unwrap_or(""), n)).collect();
    let mut todo: Vec<&str> = pkgs.values().filter(|p| matches!(p["name"].as_str(), Some("sw-app" | "sw-cli"))).filter_map(|p| p["id"].as_str()).collect();
    let mut seen = HashSet::new();
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        for d in nodes.get(id).and_then(|n| n["deps"].as_array()).into_iter().flatten() {
            // normal dependencies only (no build scripts or dev-dependencies)
            let normal = d["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"].is_null() || k["kind"] == "normal");
            if normal && let Some(p) = d["pkg"].as_str() {
                todo.push(p);
            }
        }
    }
    let mut linked: Vec<&Value> = seen
        .iter()
        .filter_map(|id| pkgs.get(id).copied())
        .filter(|p| !p["source"].is_null() || p["manifest_path"].as_str().is_some_and(|m| m.contains("third_party")))
        .collect();
    linked.sort_by_key(|p| (p["name"].as_str().unwrap_or("").to_string(), p["version"].as_str().unwrap_or("").to_string()));

    let mut out = String::from(
        "Third-party software in SampleWright\n==================================\n\n\
         SampleWright links the crates listed below; their licence files follow.\n\
         The works SampleWright is partly ported from come last (see LICENSES/README.md).\n\n",
    );
    let name = |p: &Value| format!("{} {}", p["name"].as_str().unwrap_or("?"), p["version"].as_str().unwrap_or("?"));
    let lic = |p: &Value| p["license"].as_str().unwrap_or("see licence file").to_string();
    for p in &linked {
        out += &format!("  {}  ({})\n", name(p), lic(p));
    }
    for p in &linked {
        let dir = Path::new(p["manifest_path"].as_str().unwrap_or("")).parent().map(Path::to_path_buf).unwrap_or_default();
        let mut files = license_files(&dir);
        if files.is_empty() && dir.to_string_lossy().contains("third_party") {
            // vendored workspaces keep the licence at their top level
            files = dir.ancestors().skip(1).take(3).map(license_files).find(|f| !f.is_empty()).unwrap_or_default();
        }
        out += &format!("\n{}\n{}  ({})\n{}\n", "-".repeat(78), name(p), lic(p), "-".repeat(78));
        if files.is_empty() {
            out += &format!("(no licence file in the package; licence: {})\n", lic(p));
        }
        for f in files {
            let text = fs::read_to_string(&f).unwrap_or_default();
            out += &format!("\n[{}]\n{}\n", f.file_name().unwrap_or_default().to_string_lossy(), text.trim_end());
        }
    }
    let mut derived: Vec<PathBuf> = fs::read_dir(root().join("LICENSES"))?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt")).collect();
    derived.sort();
    for f in derived {
        out += &format!(
            "\n{}\nDerived from: {}\n{}\n\n{}\n",
            "=".repeat(78),
            f.file_stem().unwrap_or_default().to_string_lossy(),
            "=".repeat(78),
            fs::read_to_string(&f)?.trim_end()
        );
    }
    Ok(out)
}

const DESKTOP: &str = "[Desktop Entry]
Type=Application
Name=SampleWright
Comment=Separate songs into stems and cut loops and one-shots for your DAW
Exec=samplewright %f
Icon=samplewright
Categories=AudioVideo;Audio;Music;
MimeType=audio/mpeg;audio/flac;audio/ogg;audio/x-wav;audio/mp4;audio/aiff;
Terminal=false
";

fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>SampleWright</string>
  <key>CFBundleDisplayName</key><string>SampleWright</string>
  <key>CFBundleIdentifier</key><string>io.github.imdabossou.samplewright</string>
  <key>CFBundleExecutable</key><string>samplewright</string>
  <key>CFBundleIconFile</key><string>samplewright</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
"#
    )
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let name = e.file_name().to_string_lossy().to_string();
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else if !name.ends_with(".part") && !name.ends_with(".ckpt") {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.extension().is_some_and(|e| e == "exe")
    }
}

fn tar_gz(dir: &Path, name: &str, archive: &Path) -> Result<()> {
    let enc = flate2::write::GzEncoder::new(fs::File::create(archive)?, flate2::Compression::default());
    let mut t = tar::Builder::new(enc);
    t.append_dir_all(name, dir)?;
    t.into_inner()?.finish()?;
    Ok(())
}

fn zip_dir(dir: &Path, base: &Path, archive: &Path) -> Result<()> {
    let mut z = zip::ZipWriter::new(fs::File::create(archive)?);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&d)?.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let rel = p.strip_prefix(base)?.to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                z.add_directory(format!("{rel}/"), zip::write::SimpleFileOptions::default())?;
                stack.push(p);
            } else {
                let mode = if is_executable(&p) { 0o755 } else { 0o644 };
                let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(mode);
                z.start_file(rel, opts)?;
                z.write_all(&fs::read(&p)?)?;
            }
        }
    }
    z.finish()?;
    Ok(())
}

fn package(args: &[String]) -> Result<()> {
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let triple = match opt("--target") {
        Some(t) => t,
        None => host_triple()?,
    };
    let plat = opt("--platform").unwrap_or_else(|| platform_of(&triple));
    let version = metadata(&["--no-deps"])?["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == "sw-app")
        .and_then(|p| p["version"].as_str().map(str::to_string))
        .context("sw-app version")?;

    let mut build = cargo();
    build.args(["build", "--release", "--locked", "-p", "sw-app", "-p", "sw-cli"]);
    if opt("--target").is_some() {
        build.args(["--target", &triple]);
    }
    let status = build.status()?;
    if !status.success() {
        bail!("cargo build failed");
    }
    let root = root();
    let bin = if opt("--target").is_some() { root.join("target").join(&triple).join("release") } else { root.join("target/release") };
    let exe = if plat.starts_with("windows") { ".exe" } else { "" };
    let app_exe = bin.join(format!("samplewright{exe}"));

    let dist = root.join("dist");
    let name = format!("samplewright-{version}-{plat}");
    let out = dist.join(&name);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out)?;

    let prog_dir = if plat.starts_with("macos") {
        let contents = out.join("SampleWright.app/Contents");
        fs::create_dir_all(contents.join("MacOS"))?;
        fs::create_dir_all(contents.join("Resources"))?;
        fs::write(contents.join("Info.plist"), info_plist(&version))?;
        if Command::new("iconutil").arg("--help").output().is_ok() {
            let iconset = dist.join("samplewright.iconset");
            fs::create_dir_all(&iconset)?;
            for s in [16u32, 32, 128, 256, 512] {
                for (scale, suffix) in [(1, ""), (2, "@2x")] {
                    let f = iconset.join(format!("icon_{s}x{s}{suffix}.png"));
                    run(Command::new(&app_exe).arg("--write-icon").arg(&f).arg((s * scale).to_string()))?;
                }
            }
            run(Command::new("iconutil").arg("-c").arg("icns").arg(&iconset).arg("-o").arg(contents.join("Resources/samplewright.icns")))?;
            fs::remove_dir_all(&iconset)?;
        } else {
            eprintln!("note: iconutil not found, the .app has no icon");
        }
        contents.join("MacOS")
    } else {
        out.clone()
    };
    for prog in ["samplewright", "samplewright-cli"] {
        fs::copy(bin.join(format!("{prog}{exe}")), prog_dir.join(format!("{prog}{exe}")))?;
    }
    for f in ["README.md", "LICENSE"] {
        fs::copy(root.join(f), out.join(f))?;
    }
    copy_dir(&root.join("LICENSES"), &out.join("LICENSES"))?;
    fs::write(out.join("THIRD_PARTY_LICENSES.txt"), third_party_licenses(&triple)?)?;
    if flag("--with-models") {
        // found next to the executable, see sw_models::model_dirs
        copy_dir(&root.join("models"), &prog_dir.join("models"))?;
    }
    if plat.starts_with("linux") {
        fs::copy(root.join("assets/samplewright.png"), out.join("samplewright.png"))?;
        fs::write(out.join("samplewright.desktop"), DESKTOP)?;
    }

    let archive = if plat.starts_with("linux") {
        let a = dist.join(format!("{name}.tar.gz"));
        tar_gz(&out, &name, &a)?;
        a
    } else {
        let a = dist.join(format!("{name}.zip"));
        zip_dir(&out, &dist, &a)?;
        a
    };
    println!("packaged {}", archive.display());

    if flag("--appimage") && plat.starts_with("linux") {
        let tool = std::env::var("APPIMAGETOOL").unwrap_or_else(|_| "appimagetool".into());
        let appdir = dist.join("samplewright.AppDir");
        let _ = fs::remove_dir_all(&appdir);
        fs::create_dir_all(appdir.join("usr/bin"))?;
        for prog in ["samplewright", "samplewright-cli"] {
            fs::copy(bin.join(prog), appdir.join("usr/bin").join(prog))?;
        }
        if flag("--with-models") {
            copy_dir(&root.join("models"), &appdir.join("usr/bin/models"))?;
        }
        fs::copy(root.join("assets/samplewright.png"), appdir.join("samplewright.png"))?;
        fs::write(appdir.join("samplewright.desktop"), DESKTOP)?;
        let apprun = appdir.join("AppRun");
        fs::write(&apprun, "#!/bin/sh\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexec \"$HERE/usr/bin/samplewright\" \"$@\"\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&apprun, fs::Permissions::from_mode(0o755))?;
        }
        let image = dist.join(format!("{name}.AppImage"));
        let arch = if plat.ends_with("arm64") { "aarch64" } else { "x86_64" };
        run(Command::new(&tool).env("ARCH", arch).arg(&appdir).arg(&image))?;
        println!("packaged {}", image.display());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("package") => package(&args[1..]),
        _ => {
            eprintln!("usage: cargo xtask package [--target <triple>] [--platform <name>] [--with-models] [--appimage]");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_dir_keeps_layout_and_modes() {
        let tmp = std::env::temp_dir().join(format!("xtask-zip-{}", std::process::id()));
        let app = tmp.join("SampleWright");
        fs::create_dir_all(app.join("models")).unwrap();
        fs::write(app.join("models/net.bin"), b"weights").unwrap();
        fs::write(app.join("samplewright"), b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(app.join("samplewright"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let archive = tmp.join("out.zip");
        zip_dir(&app, &tmp, &archive).unwrap();

        let mut z = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        let mut names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["SampleWright/models/", "SampleWright/models/net.bin", "SampleWright/samplewright"]);
        let mut data = String::new();
        std::io::Read::read_to_string(&mut z.by_name("SampleWright/models/net.bin").unwrap(), &mut data).unwrap();
        assert_eq!(data, "weights");
        assert_eq!(z.by_name("SampleWright/models/net.bin").unwrap().unix_mode().map(|m| m & 0o777), Some(0o644));
        #[cfg(unix)]
        assert_eq!(z.by_name("SampleWright/samplewright").unwrap().unix_mode().map(|m| m & 0o777), Some(0o755));
        fs::remove_dir_all(&tmp).unwrap();
    }
}
