#![allow(deprecated)]

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::prelude::*;

use crate::sni::item::SniItemState;

#[derive(Debug, Clone)]
pub struct IconPixmap {
    pub width: i32,
    pub height: i32,
    pub bytes: Vec<u8>,
}

/// Create a gtk::Image from SNI item state
pub fn make_tray_image(state: &SniItemState) -> gtk::Image {
    eprintln!(
        "[icon::make] icon_name={:?} theme_path={:?} pixmaps={}",
        state.icon_name, state.icon_theme_path, state.icon_pixmaps.len()
    );

    // 1. absolute path in icon_name — check FIRST
    if state.icon_name.starts_with('/') {
        let f = gtk::gio::File::for_path(&state.icon_name);
        if f.query_exists(gtk::gio::Cancellable::NONE) {
            eprintln!("[icon::make] absolute path file exists → FileIcon");
            return gtk::Image::from_gicon(&gtk::gio::FileIcon::new(&f));
        }
        eprintln!("[icon::make] absolute path file NOT found: {}", state.icon_name);
    }

    // 2. themed icon via GTK icon theme
    if !state.icon_name.is_empty() {
        let themed = gtk::gio::ThemedIcon::new(&state.icon_name);
        if !themed.names().is_empty() {
            eprintln!("[icon::make] themed icon name={:?}", state.icon_name);
            return gtk::Image::from_gicon(&themed);
        }
    }

    // 3. icon theme path + icon name → file path lookup
    if !state.icon_theme_path.is_empty() && !state.icon_name.is_empty() {
        for ext in &["png", "svg", "xpm"] {
            let path = format!("{}/{}.{}", state.icon_theme_path, state.icon_name, ext);
            let f = gtk::gio::File::for_path(&path);
            if f.query_exists(gtk::gio::Cancellable::NONE) {
                eprintln!("[icon::make] theme path file found: {path}");
                return gtk::Image::from_gicon(&gtk::gio::FileIcon::new(&f));
            }
        }
        eprintln!(
            "[icon::make] theme path lookup failed for {}/{}",
            state.icon_theme_path, state.icon_name
        );
    }

    // 4. pixmap fallback
    if !state.icon_pixmaps.is_empty() {
        if let Some(img) = pixmap_image(&state.icon_pixmaps) {
            eprintln!("[icon::make] pixmap image created");
            return img;
        }
        eprintln!("[icon::make] pixmap conversion failed");
    }

    // fallback
    eprintln!("[icon::make] FALLBACK — empty image");
    gtk::Image::new()
}

pub fn update_tray_image(image: &gtk::Image, state: &SniItemState) {
    // 1. absolute path
    if state.icon_name.starts_with('/') {
        let f = gtk::gio::File::for_path(&state.icon_name);
        if f.query_exists(gtk::gio::Cancellable::NONE) {
            image.set_from_gicon(&gtk::gio::FileIcon::new(&f));
            return;
        }
    }
    // 2. themed icon
    if !state.icon_name.is_empty() {
        let themed = gtk::gio::ThemedIcon::new(&state.icon_name);
        if !themed.names().is_empty() {
            image.set_from_gicon(&themed);
            return;
        }
    }
    // 3. theme path lookup
    if !state.icon_theme_path.is_empty() && !state.icon_name.is_empty() {
        for ext in &["png", "svg", "xpm"] {
            let path = format!("{}/{}.{}", state.icon_theme_path, state.icon_name, ext);
            let f = gtk::gio::File::for_path(&path);
            if f.query_exists(gtk::gio::Cancellable::NONE) {
                image.set_from_gicon(&gtk::gio::FileIcon::new(&f));
                return;
            }
        }
    }
    // 4. pixmap
    if !state.icon_pixmaps.is_empty() {
        if let Ok(pb) = pixmap_to_pixbuf(best_pixmap(&state.icon_pixmaps).unwrap()) {
            image.set_from_pixbuf(Some(&pb));
        }
    }
}

// ── pixmap helpers ────────────────────────────────────────────────

fn best_pixmap(pixmaps: &[IconPixmap]) -> Option<&IconPixmap> {
    let target = 24;
    pixmaps.iter().min_by_key(|p| (p.width - target).abs())
}

fn pixmap_image(pixmaps: &[IconPixmap]) -> Option<gtk::Image> {
    let pm = best_pixmap(pixmaps)?;
    let pb = pixmap_to_pixbuf(pm).ok()?;
    Some(gtk::Image::from_pixbuf(Some(&pb)))
}

fn pixmap_to_pixbuf(pm: &IconPixmap) -> Result<Pixbuf, ()> {
    let expected = (pm.width * pm.height * 4) as usize;
    if pm.bytes.len() < expected {
        return Err(());
    }
    let pb = Pixbuf::new(Colorspace::Rgb, true, 8, pm.width, pm.height).ok_or(())?;
    // ARGB → RGBA byte swap
    let mut rgba = Vec::with_capacity(expected);
    for chunk in pm.bytes[..expected].chunks_exact(4) {
        rgba.push(chunk[1]); // R
        rgba.push(chunk[2]); // G
        rgba.push(chunk[3]); // B
        rgba.push(chunk[0]); // A
    }
    unsafe {
        let pixels = pb.pixels();
        let dst =
            std::slice::from_raw_parts_mut(pixels.as_ptr() as *mut u8, pixels.len() * 4);
        let len = dst.len().min(rgba.len());
        dst[..len].copy_from_slice(&rgba[..len]);
    }
    Ok(pb)
}
