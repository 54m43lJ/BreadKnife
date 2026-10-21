//! Parsing and normalization of `Notify` hints.
//!
//! Only hint-derived fields live here.  Actions and the expiration timeout are
//! ordinary `Notify` arguments and are handled by the server module.
//! Protocol reference: `reference/8.hints.html` and `reference/5.icons-and-images.html`.

use std::collections::HashMap;

use zbus::zvariant::{Array, OwnedValue, Structure, Value};

use crate::types::{NoticeIcon, Urgency};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HintSet {
    pub(crate) urgency: Urgency,
    pub(crate) icon: NoticeIcon,
}

/// Normalize the hints dictionary and the `app_icon` method argument.
///
/// Unknown hints are ignored; malformed values fall back to the next icon
/// source listed in the specification.
pub(crate) fn parse(hints: &HashMap<String, OwnedValue>, app_icon: &str) -> HintSet {
    HintSet {
        urgency: hints
            .get("urgency")
            .and_then(|value| u8::try_from(value).ok())
            .map(Urgency::from_byte)
            .unwrap_or_default(),
        icon: parse_icon(hints, app_icon),
    }
}

fn parse_icon(hints: &HashMap<String, OwnedValue>, app_icon: &str) -> NoticeIcon {
    // image-data > image-path > app_icon > icon_data
    for key in ["image-data", "image_data"] {
        if let Some(icon) = hints.get(key).and_then(parse_image_data) {
            return icon;
        }
    }

    for key in ["image-path", "image_path"] {
        if let Some(path) = hints.get(key).and_then(as_text)
            && !path.is_empty()
        {
            return NoticeIcon::Name(path.to_owned());
        }
    }

    if !app_icon.is_empty() {
        return NoticeIcon::Name(app_icon.to_owned());
    }

    for key in ["icon_data", "icon-data"] {
        if let Some(icon) = hints.get(key).and_then(parse_image_data) {
            return icon;
        }
    }

    NoticeIcon::None
}

/// Parse the `(iiibiiay)` image structure and normalize it to RGBA8.
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
    let data = Vec::<u8>::try_from(Array::try_from(&fields[6]).ok()?).ok()?;

    let expected_channels = if has_alpha { 4 } else { 3 };
    if width <= 0
        || height <= 0
        || rowstride <= 0
        || bits_per_sample != 8
        || channels != expected_channels
    {
        return None;
    }

    let width = usize::try_from(width).ok()?;
    let height = usize::try_from(height).ok()?;
    let rowstride = usize::try_from(rowstride).ok()?;
    let channels = usize::try_from(channels).ok()?;

    let row_bytes = width.checked_mul(channels)?;
    if rowstride < row_bytes {
        return None;
    }

    // The final row may omit its trailing padding.
    let needed = (height - 1)
        .checked_mul(rowstride)?
        .checked_add(row_bytes)?;
    if data.len() < needed {
        return None;
    }

    let pixel_count = width.checked_mul(height)?;
    let byte_count = pixel_count.checked_mul(4)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(byte_count).ok()?;

    for row in 0..height {
        let row_start = row * rowstride;
        for column in 0..width {
            let pixel = row_start + column * channels;
            rgba.extend_from_slice(&data[pixel..pixel + 3]);
            let alpha = if channels == 4 { data[pixel + 3] } else { 0xff };
            rgba.push(alpha);
        }
    }

    Some(NoticeIcon::Pixels {
        width: i32::try_from(width).ok()?,
        height: i32::try_from(height).ok()?,
        rgba,
    })
}

fn as_text(value: &OwnedValue) -> Option<&str> {
    <&str>::try_from(value).ok()
}

fn as_i32(value: &Value<'_>) -> Option<i32> {
    i32::try_from(value).ok()
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

    fn text(value: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(value.to_owned())).unwrap()
    }

    fn image(
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
    fn defaults_are_normal_urgency_and_no_icon() {
        let set = parse(&HashMap::new(), "");
        assert_eq!(set.urgency, Urgency::Normal);
        assert_eq!(set.icon, NoticeIcon::None);
    }

    #[test]
    fn urgency_is_a_byte_and_invalid_types_fall_back_to_normal() {
        let byte = hints([("urgency", OwnedValue::from(2u8))]);
        assert_eq!(parse(&byte, "").urgency, Urgency::Critical);

        let unknown = hints([("urgency", OwnedValue::from(9u8))]);
        assert_eq!(parse(&unknown, "").urgency, Urgency::Normal);

        let wrong_type = hints([("urgency", OwnedValue::from(2u32))]);
        assert_eq!(parse(&wrong_type, "").urgency, Urgency::Normal);
    }

    #[test]
    fn icon_priority_is_image_data_path_app_icon_then_legacy_data() {
        let all = hints([
            ("image-data", image(1, 1, 4, true, 4, vec![1, 2, 3, 4])),
            ("image-path", text("/tmp/path.png")),
            ("icon_data", image(1, 1, 3, false, 3, vec![9, 9, 9])),
        ]);
        assert_eq!(
            parse(&all, "app-icon").icon,
            NoticeIcon::Pixels {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            }
        );

        let without_data = hints([
            ("image-path", text("/tmp/path.png")),
            ("icon_data", image(1, 1, 3, false, 3, vec![9, 9, 9])),
        ]);
        assert_eq!(
            parse(&without_data, "app-icon").icon,
            NoticeIcon::Name("/tmp/path.png".into())
        );

        let app_icon_wins_over_legacy_data =
            hints([("icon_data", image(1, 1, 3, false, 3, vec![7, 8, 9]))]);
        assert_eq!(
            parse(&app_icon_wins_over_legacy_data, "app-icon").icon,
            NoticeIcon::Name("app-icon".into())
        );

        let only_app_icon = HashMap::new();
        assert_eq!(
            parse(&only_app_icon, "app-icon").icon,
            NoticeIcon::Name("app-icon".into())
        );

        let legacy = hints([("icon_data", image(1, 1, 3, false, 3, vec![7, 8, 9]))]);
        assert_eq!(
            parse(&legacy, "").icon,
            NoticeIcon::Pixels {
                width: 1,
                height: 1,
                rgba: vec![7, 8, 9, 255],
            }
        );
    }

    #[test]
    fn malformed_image_data_falls_back_to_the_next_source() {
        let set = hints([
            ("image-data", image(1, 1, 4, true, 3, vec![1, 2, 3])),
            ("image_path", text("fallback-icon")),
        ]);
        assert_eq!(
            parse(&set, "").icon,
            NoticeIcon::Name("fallback-icon".into())
        );
    }

    #[test]
    fn image_data_is_normalized_to_rgba_and_padding_is_removed() {
        // 2×1, three channels, rowstride 8 (two bytes of trailing padding).
        let set = hints([(
            "image-data",
            image(2, 1, 8, false, 3, vec![255, 0, 0, 0, 255, 0, 9, 9]),
        )]);
        assert_eq!(
            parse(&set, "").icon,
            NoticeIcon::Pixels {
                width: 2,
                height: 1,
                rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
            }
        );
    }

    #[test]
    fn image_data_may_omit_the_last_row_padding() {
        let set = hints([(
            "image-data",
            image(1, 2, 8, false, 3, vec![1, 2, 3, 9, 9, 9, 9, 9, 4, 5, 6]),
        )]);
        assert_eq!(
            parse(&set, "").icon,
            NoticeIcon::Pixels {
                width: 1,
                height: 2,
                rgba: vec![1, 2, 3, 255, 4, 5, 6, 255],
            }
        );
    }

    #[test]
    fn image_data_honours_the_alpha_channel() {
        let set = hints([("image-data", image(1, 1, 4, true, 4, vec![1, 2, 3, 4]))]);
        assert_eq!(
            parse(&set, "").icon,
            NoticeIcon::Pixels {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            }
        );
    }

    #[test]
    fn four_channels_without_alpha_are_rejected() {
        let set = hints([
            ("image-data", image(1, 1, 4, false, 4, vec![1, 2, 3, 4])),
            ("image_path", text("fallback-icon")),
        ]);
        assert_eq!(
            parse(&set, "").icon,
            NoticeIcon::Name("fallback-icon".into())
        );
    }

    #[test]
    fn image_data_with_a_non_specification_numeric_type_is_rejected() {
        let structure = StructureBuilder::new()
            .add_field(1u32)
            .add_field(1i32)
            .add_field(3i32)
            .add_field(false)
            .add_field(8i32)
            .add_field(3i32)
            .add_field(Array::from(vec![1u8, 2, 3]))
            .build()
            .unwrap();
        let set = hints([
            ("image-data", OwnedValue::try_from(structure).unwrap()),
            ("image_path", text("fallback-icon")),
        ]);
        assert_eq!(
            parse(&set, "").icon,
            NoticeIcon::Name("fallback-icon".into())
        );
    }
}
