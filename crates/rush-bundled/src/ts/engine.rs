//! Which TypeScript language server runs for a checkout, and where the
//! checkout is.
//! Ported from internal/bundled/ts/engine.go:1-163.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A TypeScript language server: which one, and its binary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Engine {
    /// "tsrs" or "tsgo"
    pub name: String,
    pub bin: String,
}

/// The suffix npm's platform packages use: darwin-arm64, linux-x64…
pub fn platform() -> String {
    let arch = match rush_plugin::GOARCH {
        "amd64" => "x64",
        a => a,
    };
    format!("{}-{arch}", rush_plugin::GOOS)
}

/// Where npm and pnpm install packages for everyone, since the plugin's
/// PATH holds only the system's folders.
fn global_modules() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    vec![
        PathBuf::from("/opt/homebrew/lib/node_modules"),
        PathBuf::from("/usr/local/lib/node_modules"),
        home.join(".npm-global").join("lib").join("node_modules"),
        home.join("Library")
            .join("pnpm")
            .join("global")
            .join("5")
            .join("node_modules"),
        home.join(".local")
            .join("share")
            .join("pnpm")
            .join("global")
            .join("5")
            .join("node_modules"),
    ]
}

/// The engine for the project at root, as want ("auto", "tsrs" or
/// "tsgo") asks: tsrs first for auto, the project's own install before a
/// global one. The platform binary is run, never a package's Node shim,
/// so the process rush watches is the server itself.
pub fn resolve(root: &str, want: &str) -> Result<Engine, String> {
    if want != "tsgo" {
        if let Some(bin) = find(root, tsrs_at) {
            return Ok(Engine {
                name: "tsrs".into(),
                bin,
            });
        }
        if want == "tsrs" {
            return Err(format!(
                "no tsrs for {}: npm i -g @maschwenk/tsrs, or set the engine to auto",
                platform()
            ));
        }
    }
    if let Some(bin) = find(root, tsgo_at) {
        return Ok(Engine {
            name: "tsgo".into(),
            bin,
        });
    }
    Err(
        "no TypeScript language server: install @maschwenk/tsrs (npm i -g), \
         or @typescript/native-preview or typescript 7 in the project"
            .into(),
    )
}

/// The first binary at, given a node_modules folder, finds: in root's and
/// each folder above it, then the global ones.
fn find(root: &str, at: fn(&Path) -> Option<String>) -> Option<String> {
    let mut dir = PathBuf::from(root);
    loop {
        if let Some(bin) = at(&dir.join("node_modules")) {
            return Some(bin);
        }
        match dir.parent() {
            Some(p) if p != dir => dir = p.to_path_buf(),
            _ => break,
        }
    }
    global_modules().iter().find_map(|m| at(m))
}

/// tsrs's platform binary for modules/@maschwenk/tsrs: beside the package
/// where pnpm keeps it, under it where npm nests it, or at the top of
/// modules where npm hoists it.
fn tsrs_at(modules: &Path) -> Option<String> {
    let pkg = format!("tsrs-{}", platform());
    siblings(
        &modules.join("@maschwenk").join("tsrs"),
        &[
            Path::new("..").join(&pkg).join("tsrs"),
            Path::new("node_modules")
                .join("@maschwenk")
                .join(&pkg)
                .join("tsrs"),
        ],
    )
    .or_else(|| first_file(&[modules.join("@maschwenk").join(&pkg).join("tsrs")]))
}

/// tsgo's platform binary: the native preview's, or TypeScript 7's own
/// (the same engine, released as tsc).
fn tsgo_at(modules: &Path) -> Option<String> {
    let preview = format!("native-preview-{}", platform());
    let ts7 = format!("typescript-{}", platform());
    siblings(
        &modules.join("@typescript").join("native-preview"),
        &[
            Path::new("..").join(&preview).join("lib").join("tsgo"),
            Path::new("node_modules")
                .join("@typescript")
                .join(&preview)
                .join("lib")
                .join("tsgo"),
        ],
    )
    .or_else(|| {
        first_file(&[modules
            .join("@typescript")
            .join(&preview)
            .join("lib")
            .join("tsgo")])
    })
    .or_else(|| {
        siblings(
            &modules.join("typescript"),
            &[
                Path::new("..")
                    .join("@typescript")
                    .join(&ts7)
                    .join("lib")
                    .join("tsc"),
                Path::new("node_modules")
                    .join("@typescript")
                    .join(&ts7)
                    .join("lib")
                    .join("tsc"),
            ],
        )
    })
    .or_else(|| {
        first_file(&[modules
            .join("@typescript")
            .join(&ts7)
            .join("lib")
            .join("tsc")])
    })
}

/// rel joined to the package at pkg with its links resolved, for each
/// rel; None when there's no package.
fn siblings(pkg: &Path, rels: &[PathBuf]) -> Option<String> {
    let real = std::fs::canonicalize(pkg).ok()?;
    rels.iter()
        .map(|rel| clean(&real.join(rel)))
        .find(|p| is_file(p))
        .map(|p| p.to_string_lossy().into_owned())
}

/// filepath.Join's cleaning: ".." takes the folder before it away.
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

fn first_file(paths: &[PathBuf]) -> Option<String> {
    paths
        .iter()
        .find(|p| is_file(p))
        .map(|p| p.to_string_lossy().into_owned())
}

fn is_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The root of the checkout file is in: the nearest folder above it with
/// a .git (a worktree's is a file), else its nearest tsconfig.json's; ""
/// for neither. One server answers for the whole checkout: opened files
/// find their own projects inside it, and share what they load, where a
/// server per tsconfig would load the shared packages once each.
pub fn workspace_root(file: &str) -> String {
    let mut dir = PathBuf::from(file);
    if !std::fs::metadata(file).is_ok_and(|m| m.is_dir()) {
        dir = dir.parent().map(Path::to_path_buf).unwrap_or_default();
    }
    let mut tsconfig = String::new();
    loop {
        if std::fs::metadata(dir.join(".git")).is_ok() {
            return dir.to_string_lossy().into_owned();
        }
        if tsconfig.is_empty() && std::fs::metadata(dir.join("tsconfig.json")).is_ok() {
            tsconfig = dir.to_string_lossy().into_owned();
        }
        match dir.parent() {
            Some(p) if p != dir && !p.as_os_str().is_empty() => dir = p.to_path_buf(),
            _ => return tsconfig,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ts_test.go TestWorkspaceRoot: one server per checkout: the root is
    // the .git's folder, not the nearest tsconfig's, so a monorepo's
    // packages share one.
    #[test]
    fn workspace_roots() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let pkg = root.join("packages").join("a").join("src");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(root.join(".git"), "gitdir: elsewhere\n").unwrap();
        std::fs::write(root.join("packages").join("a").join("tsconfig.json"), "{}").unwrap();
        let f = pkg.join("x.ts");
        std::fs::write(&f, "").unwrap();
        assert_eq!(workspace_root(f.to_str().unwrap()), root.to_str().unwrap());
        std::fs::remove_file(root.join(".git")).unwrap();
        assert_eq!(
            workspace_root(f.to_str().unwrap()),
            root.join("packages").join("a").to_str().unwrap(),
            "without a .git, the tsconfig's folder"
        );
    }

    // ts_test.go TestResolveFindsThePlatformBinary: the platform binary,
    // never the package's Node shim, wherever pnpm or npm put it.
    #[test]
    fn resolve_finds_the_platform_binary() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let store = root
            .join("node_modules")
            .join(".pnpm")
            .join("x")
            .join("node_modules")
            .join("@maschwenk");
        let bin = store.join(format!("tsrs-{}", platform())).join("tsrs");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::create_dir_all(store.join("tsrs").join("bin")).unwrap();
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(root.join("node_modules").join("@maschwenk")).unwrap();
        std::os::unix::fs::symlink(
            store.join("tsrs"),
            root.join("node_modules").join("@maschwenk").join("tsrs"),
        )
        .unwrap();
        let e = resolve(root.to_str().unwrap(), "auto").unwrap();
        assert_eq!(e.name, "tsrs");
        assert_eq!(e.bin, bin.to_str().unwrap());
    }
}
