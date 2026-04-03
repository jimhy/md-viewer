fn main() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("icon.ico");
    res.set("ProductName", "MD Viewer");
    res.set("FileDescription", "Markdown Viewer");
    res.compile().unwrap();
}
