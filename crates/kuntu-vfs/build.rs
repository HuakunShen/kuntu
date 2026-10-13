fn main() {
  let mut build = cc::Build::new();
  build.file("src/vfs.c");
  if std::env::var_os("CARGO_FEATURE_TEST_HOOKS").is_some() {
    build.define("KUNTU_TEST_HOOKS", None);
  }
  build.compile("kuntu_vfs");
  println!("cargo:rerun-if-changed=src/vfs.c");
}
