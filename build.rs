use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-changed=resources/app.rc.in");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let version = env::var("CARGO_PKG_VERSION").expect("package version");
    let mut parts = version
        .split('.')
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .collect::<Vec<_>>();
    parts.resize(4, 0);

    let template = fs::read_to_string(manifest_dir.join("resources/app.rc.in"))
        .expect("read Windows resource template");
    let icon_path = manifest_dir
        .join("assets/app-icon.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let resource = template
        .replace("@ICON_PATH@", &icon_path)
        .replace(
            "@VERSION_COMMAS@",
            &format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]),
        )
        .replace(
            "@VERSION_DOTS@",
            &format!("{}.{}.{}.{}", parts[0], parts[1], parts[2], parts[3]),
        );
    let rc_path = out_dir.join("app.rc");
    fs::write(&rc_path, resource).expect("write generated Windows resource");

    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("embed Windows resources");
}
