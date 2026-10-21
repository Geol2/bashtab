fn main() {
    println!("cargo:rerun-if-changed=res/bashtab.rc");
    println!("cargo:rerun-if-changed=icon.ico");
    println!("cargo:rerun-if-changed=fast-rc.sh");
    embed_resource::compile("res/bashtab.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
