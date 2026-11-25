fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=fonts");
    slint_build::compile("ui/panel.slint").expect("compile ui/panel.slint");
}
