use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon_path = out.join("relay.ico");
    fs::write(&icon_path, icon()).expect("write generated application icon");
    winresource::WindowsResource::new()
        .set_icon(icon_path.to_str().expect("icon path"))
        .set_manifest_file("app.manifest")
        .set("ProductName", "随手传 LAN Relay")
        .set("FileDescription", "随手传 Rust 桌面应用")
        .compile()
        .expect("compile Windows application resources");
}

fn icon() -> Vec<u8> {
    const SIZE: u32 = 32;
    let mut image = Vec::new();
    image.extend_from_slice(&40u32.to_le_bytes());
    image.extend_from_slice(&(SIZE as i32).to_le_bytes());
    image.extend_from_slice(&((SIZE * 2) as i32).to_le_bytes());
    image.extend_from_slice(&1u16.to_le_bytes());
    image.extend_from_slice(&32u16.to_le_bytes());
    image.extend_from_slice(&[0; 24]);
    for y in (0..SIZE).rev() {
        for x in 0..SIZE {
            let right = (8..24).contains(&x) && (9..12).contains(&y)
                || (19..26).contains(&x)
                    && (6..15).contains(&y)
                    && (x as i32 - 19) >= (y as i32 - 10).abs();
            let left = (8..24).contains(&x) && (20..23).contains(&y)
                || (6..13).contains(&x)
                    && (17..26).contains(&y)
                    && (12 - x as i32) >= (y as i32 - 21).abs();
            image.extend_from_slice(if left || right {
                &[255, 255, 255, 255]
            } else {
                &[98, 119, 83, 255]
            });
        }
    }
    image.extend_from_slice(&[0; 128]);
    let mut ico = vec![0, 0, 1, 0, 1, 0, 32, 32, 0, 0, 1, 0, 32, 0];
    ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend(image);
    ico
}
