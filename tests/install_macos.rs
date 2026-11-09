//! The bundle swap on real bundles: built, ad-hoc sealed and zipped the way a release is.
#![cfg(target_os = "macos")]

use std::path::Path;
use std::process::Command;

use signed_update::install::install_bundle;

/// `<dir>/Foo.app` holding `marker` in its Resources, sealed (or, with `seal` false, left unsigned).
fn make_bundle(dir: &Path, marker: &str, seal: bool) -> std::path::PathBuf {
    let app = dir.join("Foo.app");
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
    std::fs::write(
        app.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>foo</string>
<key>CFBundleIdentifier</key><string>test.foo</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#,
    )
    .unwrap();
    std::fs::copy("/usr/bin/true", app.join("Contents/MacOS/foo")).unwrap();
    std::fs::write(app.join("Contents/Resources/marker"), marker).unwrap();
    if seal {
        let status = Command::new("codesign").args(["-f", "-s", "-"]).arg(&app).output().unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    }
    app
}

fn zip(app: &Path, zip: &Path) {
    let status = Command::new("ditto").args(["-c", "-k", "--keepParent", "--sequesterRsrc"]).arg(app).arg(zip).status().unwrap();
    assert!(status.success());
}

fn marker(app: &Path) -> String {
    std::fs::read_to_string(app.join("Contents/Resources/marker")).unwrap()
}

#[test]
fn swaps_in_the_new_bundle_and_leaves_no_staging_behind() {
    let dir = tempfile::tempdir().unwrap();
    let (new_dir, installed_dir) = (dir.path().join("new"), dir.path().join("Applications"));
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::create_dir_all(&installed_dir).unwrap();
    let new = make_bundle(&new_dir, "new", true);
    let installed = make_bundle(&installed_dir, "old", true);
    zip(&new, &dir.path().join("update.zip"));
    install_bundle(&dir.path().join("update.zip"), &installed).unwrap();
    assert_eq!(marker(&installed), "new");
    let leftovers: Vec<_> = std::fs::read_dir(&installed_dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(leftovers, ["Foo.app"], "the staging folder, holding the old app, is removed");
}

#[test]
fn a_broken_seal_is_refused_and_the_installed_app_is_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (new_dir, installed_dir) = (dir.path().join("new"), dir.path().join("Applications"));
    std::fs::create_dir_all(&new_dir).unwrap();
    std::fs::create_dir_all(&installed_dir).unwrap();
    let new = make_bundle(&new_dir, "tampered", true);
    let installed = make_bundle(&installed_dir, "old", true);
    // Changed after sealing: the seal no longer covers what is inside.
    std::fs::write(new.join("Contents/Resources/marker"), "changed after sealing").unwrap();
    zip(&new, &dir.path().join("update.zip"));
    let error = install_bundle(&dir.path().join("update.zip"), &installed).unwrap_err().to_string();
    assert!(error.contains("seal"), "{error}");
    assert_eq!(marker(&installed), "old");
}

#[test]
fn a_zip_without_the_bundle_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let installed_dir = dir.path().join("Applications");
    std::fs::create_dir_all(&installed_dir).unwrap();
    let installed = make_bundle(&installed_dir, "old", true);
    let other = dir.path().join("Other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("readme"), "no app here").unwrap();
    let archive = dir.path().join("update.zip");
    let status = Command::new("ditto").args(["-c", "-k", "--keepParent"]).arg(&other).arg(&archive).status().unwrap();
    assert!(status.success());
    let error = install_bundle(&archive, &installed).unwrap_err().to_string();
    assert!(error.contains("Foo.app"), "{error}");
    assert_eq!(marker(&installed), "old");
}
