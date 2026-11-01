//! Gives the exe its icon and the name Task Manager shows.

fn main() {
    println!("cargo:rerun-if-changed=assets/drop.ico");
    winresource::WindowsResource::new()
        .set_icon("assets/drop.ico")
        .set("FileDescription", "Drop")
        .set("ProductName", "Drop")
        .compile()
        .expect("compile the Windows resources");
}
