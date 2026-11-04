use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(embedded_tectonic)");
    println!("cargo:rerun-if-env-changed=NSPIRE_PDF_EMBED_TECTONIC");

    if let Some(path) = env::var_os("NSPIRE_PDF_EMBED_TECTONIC") {
        assert_eq!(
            env::var("CARGO_CFG_TARGET_OS").as_deref(),
            Ok("windows"),
            "embedding Tectonic is supported for Windows builds"
        );
        let path = PathBuf::from(path)
            .canonicalize()
            .expect("NSPIRE_PDF_EMBED_TECTONIC must point to tectonic.exe");
        assert!(
            path.is_file(),
            "embedded Tectonic must be an executable file"
        );
        let path = path
            .to_str()
            .expect("the embedded Tectonic path must be Unicode");
        println!("cargo:rerun-if-changed={path}");
        println!("cargo:rustc-cfg=embedded_tectonic");
        println!("cargo:rustc-env=NSPIRE_PDF_TECTONIC_PATH={path}");
    }
}
