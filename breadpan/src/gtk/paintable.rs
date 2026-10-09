//! 渲染直出：主题命中 → 主题图标；未命中 → 位图纹理。
//!
//! 须在 GTK 主线程调用（IconTheme 按显示连接取用）。

use gtk_crate::gdk;
use gtk_crate::glib;
use gtk_crate::prelude::*;
use gtk_crate::{IconLookupFlags, IconTheme, TextDirection};

use crate::{IconSource, Pixmap};

/// 查找尺寸（图标主题逻辑像素）。
const LOOKUP_SIZE: i32 = 48;
/// 主题未命中时的占位图标（Icon Naming Spec 保留名）。
const MISSING_ICON: &str = "image-missing";

/// 打包生成可渲染的 GTK 对象：主题命中 → 主题图标；未命中 → 位图纹理。
pub fn icon_paintable(icon: &IconSource) -> gdk::Paintable {
    match icon {
        IconSource::Name(name) => match lookup_theme_icon(name) {
            Some(paintable) => paintable,
            None => lookup_theme_icon(MISSING_ICON)
                .expect("image-missing is guaranteed by the icon theme spec"),
        },
        IconSource::Pixmap(pixmap) => pixmap_to_paintable(pixmap),
    }
}

/// Icon Theme Spec 命中判定（含继承主题链）。
fn lookup_theme_icon(name: &str) -> Option<gdk::Paintable> {
    let display = gdk::Display::default()?;
    let theme = IconTheme::for_display(&display);
    if !theme.has_icon(name) {
        return None;
    }
    let paintable = theme.lookup_icon(
        name,
        &[],
        LOOKUP_SIZE,
        1,
        TextDirection::Ltr,
        IconLookupFlags::empty(),
    );
    Some(paintable.upcast())
}

/// ARGB32（网络字节序，字节序 [A,R,G,B]）→ 位图纹理。
/// 色彩处理（与 gdk 内存格式的字节序对齐）在此完成；SNI 位图为未预乘数据，
/// 对应 gdk 的非预乘 A8R8G8B8 内存格式，由渲染器负责合成。
fn pixmap_to_paintable(p: &Pixmap) -> gdk::Paintable {
    let stride = p.width as usize * 4;
    let complete = p.width > 0 && p.height > 0 && p.argb.len() >= stride * p.height as usize;
    if !complete {
        // 尺寸不完整的位图 → 1×1 全透明纹理（不 panic，渲染为空）
        let empty = glib::Bytes::from_owned([0u8, 0, 0, 0]);
        return gdk::MemoryTexture::new(1, 1, gdk::MemoryFormat::A8r8g8b8, &empty, 4).upcast();
    }
    let bytes = glib::Bytes::from_owned(p.argb.clone());
    gdk::MemoryTexture::new(
        p.width as i32,
        p.height as i32,
        gdk::MemoryFormat::A8r8g8b8,
        &bytes,
        stride,
    )
    .upcast()
}
