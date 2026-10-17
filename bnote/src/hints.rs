//! hints 解析与归一：紧急度、动作、图标、超时。
//!
//! 规范条文见 `bnote/reference/notification-spec.clean.html`；本模块不保存任何状态。

use std::collections::HashMap;

use zbus::zvariant::{Array, OwnedValue, Structure, Value};

use crate::types::{Action, NoticeIcon, Urgency};

/// 一次 `Notify` 调用归一后的 hint 部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HintSet {
    pub urgency: Urgency,
    pub actions: Vec<Action>,
    pub icon: NoticeIcon,
    /// `None`＝客户端未指定（规范 -1），由宿主决定；`Some(0)`＝常驻；其余为毫秒。
    pub expire_timeout_ms: Option<u32>,
}

/// 归一 `Notify` 的 hints 参数、动作参数与超时参数。
///
/// 图标优先级：`image-data` 系列位图 → `image-path` 系列路径 → `app_icon`。
pub fn parse(
    hints: &HashMap<String, OwnedValue>,
    app_icon: &str,
    actions: &[String],
    expire_timeout: i32,
) -> HintSet {
    HintSet {
        urgency: hints
            .get("urgency")
            .and_then(parse_urgency)
            .unwrap_or_default(),
        actions: parse_actions(actions),
        icon: parse_icon(hints, app_icon),
        expire_timeout_ms: parse_expire_timeout(expire_timeout),
    }
}

fn parse_urgency(value: &OwnedValue) -> Option<Urgency> {
    let raw = as_u8(value)?;
    match raw {
        0 => Some(Urgency::Low),
        1 => Some(Urgency::Normal),
        2 => Some(Urgency::Critical),
        _ => None,
    }
}

fn parse_actions(actions: &[String]) -> Vec<Action> {
    let (pairs, _) = actions.as_chunks::<2>();
    pairs
        .iter()
        .map(|pair| Action {
            key: pair[0].clone(),
            label: pair[1].clone(),
        })
        .collect()
}

fn parse_icon(hints: &HashMap<String, OwnedValue>, app_icon: &str) -> NoticeIcon {
    for key in ["image-data", "image_data", "icon_data", "icon-data"] {
        if let Some(icon) = hints.get(key).and_then(parse_image_data) {
            return icon;
        }
    }

    for key in ["image-path", "image_path"] {
        if let Some(path) = hints.get(key).and_then(|value| as_text(value))
            && !path.is_empty()
        {
            return NoticeIcon::Name(path.to_owned());
        }
    }

    if app_icon.is_empty() {
        NoticeIcon::None
    } else {
        NoticeIcon::Name(app_icon.to_owned())
    }
}

/// 解析规范 `(iiibiiay)` 位图，并归一为紧密排列的 RGBA8。
fn parse_image_data(value: &OwnedValue) -> Option<NoticeIcon> {
    let value = Value::try_from(value).ok()?;
    let structure = value.downcast::<Structure<'_>>().ok()?;
    let fields = structure.fields();
    if fields.len() < 7 {
        return None;
    }

    let width = as_i32(&fields[0])?;
    let height = as_i32(&fields[1])?;
    let rowstride = as_i32(&fields[2])?;
    let has_alpha = bool::try_from(&fields[3]).ok()?;
    let bits_per_sample = as_i32(&fields[4])?;
    let channels = as_i32(&fields[5])?;
    let raw = Vec::<u8>::try_from(Array::try_from(&fields[6]).ok()?).ok()?;

    if width <= 0
        || height <= 0
        || rowstride <= 0
        || bits_per_sample != 8
        || !(channels == 3 || channels == 4)
    {
        return None;
    }

    let width = usize::try_from(width).ok()?;
    let height = usize::try_from(height).ok()?;
    let rowstride = usize::try_from(rowstride).ok()?;
    let channels = usize::try_from(channels).ok()?;
    let row_bytes = width.checked_mul(channels)?;
    // 最后一行可能不带尾部填充；只要求数据覆盖到最后一行像素末尾。
    let needed = (height - 1)
        .checked_mul(rowstride)?
        .checked_add(row_bytes)?;
    if rowstride < row_bytes || raw.len() < needed {
        return None;
    }

    let mut rgba = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for row in 0..height {
        let row_start = row * rowstride;
        for column in 0..width {
            let pixel = row_start + column * channels;
            rgba.extend_from_slice(&raw[pixel..pixel + 3]);
            let alpha = if channels == 4 && has_alpha {
                raw[pixel + 3]
            } else {
                0xff
            };
            rgba.push(alpha);
        }
    }

    Some(NoticeIcon::Pixels {
        width: i32::try_from(width).ok()?,
        height: i32::try_from(height).ok()?,
        rgba,
    })
}

fn parse_expire_timeout(expire_timeout: i32) -> Option<u32> {
    if expire_timeout < 0 {
        None
    } else {
        Some(expire_timeout as u32)
    }
}

fn as_text<'a>(value: &'a Value<'_>) -> Option<&'a str> {
    <&str>::try_from(value).ok()
}

fn as_u8(value: &Value<'_>) -> Option<u8> {
    u8::try_from(value)
        .ok()
        .or_else(|| u32::try_from(value).ok().and_then(|n| u8::try_from(n).ok()))
        .or_else(|| i32::try_from(value).ok().and_then(|n| u8::try_from(n).ok()))
        .or_else(|| i64::try_from(value).ok().and_then(|n| u8::try_from(n).ok()))
}

fn as_i32(value: &Value<'_>) -> Option<i32> {
    i32::try_from(value)
        .ok()
        .or_else(|| {
            u32::try_from(value)
                .ok()
                .and_then(|n| i32::try_from(n).ok())
        })
        .or_else(|| {
            i64::try_from(value)
                .ok()
                .and_then(|n| i32::try_from(n).ok())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{StructureBuilder, Value};

    fn hints<const N: usize>(entries: [(&str, OwnedValue); N]) -> HashMap<String, OwnedValue> {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect()
    }

    fn text_value(value: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(value.to_owned())).unwrap()
    }

    fn image_value(
        width: i32,
        height: i32,
        rowstride: i32,
        has_alpha: bool,
        channels: i32,
        data: Vec<u8>,
    ) -> OwnedValue {
        let structure = StructureBuilder::new()
            .add_field(width)
            .add_field(height)
            .add_field(rowstride)
            .add_field(has_alpha)
            .add_field(8i32)
            .add_field(channels)
            .add_field(Array::from(data))
            .build()
            .unwrap();
        OwnedValue::try_from(structure).unwrap()
    }

    #[test]
    fn defaults_are_normal_and_no_icon() {
        let set = parse(&HashMap::new(), "", &[], -1);
        assert_eq!(set.urgency, Urgency::Normal);
        assert_eq!(set.icon, NoticeIcon::None);
        assert!(set.actions.is_empty());
        assert_eq!(set.expire_timeout_ms, None);
    }

    #[test]
    fn urgency_accepts_byte_and_integer_encodings() {
        let byte = hints([("urgency", OwnedValue::from(2u8))]);
        assert_eq!(parse(&byte, "", &[], 0).urgency, Urgency::Critical);

        let integer = hints([("urgency", OwnedValue::from(0i32))]);
        assert_eq!(parse(&integer, "", &[], 0).urgency, Urgency::Low);

        let unknown = hints([("urgency", OwnedValue::from(9u8))]);
        assert_eq!(parse(&unknown, "", &[], 0).urgency, Urgency::Normal);
    }

    #[test]
    fn icon_falls_back_from_image_path_to_app_icon() {
        let by_path = hints([("image-path", text_value("/tmp/icon.png"))]);
        assert_eq!(
            parse(&by_path, "app-icon", &[], 0).icon,
            NoticeIcon::Name("/tmp/icon.png".into())
        );

        assert_eq!(
            parse(&HashMap::new(), "app-icon", &[], 0).icon,
            NoticeIcon::Name("app-icon".into())
        );
    }

    #[test]
    fn image_data_is_normalized_to_rgba_with_row_padding() {
        // 2×1、3 通道、rowstride 8（每行末尾 2 字节填充）。
        let set = parse(
            &hints([(
                "image-data",
                image_value(2, 1, 8, false, 3, vec![255, 0, 0, 0, 255, 0, 9, 9]),
            )]),
            "app-icon",
            &[],
            0,
        );
        assert_eq!(
            set.icon,
            NoticeIcon::Pixels {
                width: 2,
                height: 1,
                rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
            }
        );
    }

    #[test]
    fn image_data_may_omit_trailing_row_padding() {
        let set = parse(
            &hints([(
                "image-data",
                image_value(1, 2, 8, false, 3, vec![1, 2, 3, 9, 9, 9, 9, 9, 4, 5, 6]),
            )]),
            "",
            &[],
            0,
        );
        assert_eq!(
            set.icon,
            NoticeIcon::Pixels {
                width: 1,
                height: 2,
                rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
            }
        );
    }

    #[test]
    fn image_data_honours_alpha_channel() {
        let set = parse(
            &hints([(
                "image-data",
                image_value(1, 1, 4, true, 4, vec![1, 2, 3, 4]),
            )]),
            "",
            &[],
            0,
        );
        assert_eq!(
            set.icon,
            NoticeIcon::Pixels {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            }
        );
    }

    #[test]
    fn timeout_normalization_follows_spec() {
        assert_eq!(parse(&HashMap::new(), "", &[], -1).expire_timeout_ms, None);
        assert_eq!(
            parse(&HashMap::new(), "", &[], 0).expire_timeout_ms,
            Some(0)
        );
        assert_eq!(
            parse(&HashMap::new(), "", &[], 5000).expire_timeout_ms,
            Some(5000)
        );
    }

    #[test]
    fn actions_are_pairs_and_a_trailing_key_is_ignored() {
        let actions = vec![
            "default".to_owned(),
            "Open".to_owned(),
            "dismiss".to_owned(),
            "Close".to_owned(),
            "orphan".to_owned(),
        ];
        let set = parse(&HashMap::new(), "", &actions, 0);
        assert_eq!(
            set.actions,
            vec![
                Action {
                    key: "default".into(),
                    label: "Open".into(),
                },
                Action {
                    key: "dismiss".into(),
                    label: "Close".into(),
                },
            ]
        );
    }
}
