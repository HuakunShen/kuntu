fn main() {
  cc::Build::new().file("src/vfs.c").compile("kuntu_vfs");
  println!("cargo:rerun-if-changed=src/vfs.c");
}
