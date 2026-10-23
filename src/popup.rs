//! Popup menu rows and placing the popup on the right output so it stays visible.
//!
//! Accelerators are matched by this module rather than by GTK mnemonics: the
//! popup is opened from D-Bus, so it has no input event to hand GTK, and GTK's
//! own menu key handling stays inert without one. See `ui.rs`.

use super::shorten_label;

/// How a history row should appear in the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItemSpec {
    /// Digit `1`–`9` then letter `a`–`z` for the first 35 rows.
    pub accelerator: Option<char>,
    /// The history text, shortened for display.
    pub text: String,
}

/// The key that selects the row at `index`, where 0 is the newest entry.
pub fn accelerator_for(index: usize) -> Option<char> {
    match index {
        0..=8 => Some(char::from(b'1' + index as u8)),
        9..=34 => Some(char::from(b'a' + (index as u8 - 9))),
        _ => None,
    }
}

/// The row `key` selects, if it is an accelerator and that row exists.
pub fn index_for_accelerator(key: char, rows: usize) -> Option<usize> {
    let key = key.to_ascii_lowercase();
    let index = match key {
        '1'..='9' => key as usize - '1' as usize,
        'a'..='z' => key as usize - 'a' as usize + 9,
        _ => return None,
    };
    (index < rows).then_some(index)
}

pub fn menu_item_spec(index: usize, text: &str, label_max: usize) -> MenuItemSpec {
    MenuItemSpec {
        accelerator: accelerator_for(index),
        text: shorten_label(text, label_max),
    }
}

/// Geometry of one output in the compositor's global layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputGeom {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl OutputGeom {
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Maps a global pointer position onto `outputs`.
///
/// Hits the first output whose rectangle contains the point. If none does,
/// `primary` is used and the local coordinates are clamped to its origin
/// rather than going negative.
pub fn position_on_outputs(
    x: i32,
    y: i32,
    outputs: &[OutputGeom],
    primary: usize,
) -> Option<(usize, i32, i32)> {
    if outputs.is_empty() {
        return None;
    }
    for (index, output) in outputs.iter().enumerate() {
        if output.contains(x, y) {
            return Some((index, x - output.x, y - output.y));
        }
    }
    let index = primary.min(outputs.len() - 1);
    let output = outputs[index];
    Some((index, (x - output.x).max(0), (y - output.y).max(0)))
}

/// Maps a global pointer position and popup size onto `outputs`.
///
/// Picks the output the same way as [`position_on_outputs`], then places the
/// popup so it prefers growing down and to the right from the pointer. If that
/// would leave the output, the box flips so the opposite edge sits on the
/// pointer; it then slides to stay inside the output. A popup larger than the
/// output is pinned to the origin.
pub fn place_on_outputs(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    outputs: &[OutputGeom],
    primary: usize,
) -> Option<(usize, i32, i32)> {
    let (index, local_x, local_y) = position_on_outputs(x, y, outputs, primary)?;
    let output = outputs[index];
    Some((
        index,
        place_axis(local_x, width, output.width),
        place_axis(local_y, height, output.height),
    ))
}

fn place_axis(local: i32, size: i32, output_size: i32) -> i32 {
    let size = size.max(0);
    let mut pos = local;
    if pos + size > output_size {
        pos = local - size;
    }
    if size >= output_size {
        0
    } else {
        pos.clamp(0, output_size - size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_nine_rows_use_digits() {
        assert_eq!(accelerator_for(0), Some('1'));
        assert_eq!(accelerator_for(8), Some('9'));
    }

    #[test]
    fn later_rows_use_letters() {
        assert_eq!(accelerator_for(9), Some('a'));
        assert_eq!(accelerator_for(34), Some('z'));
    }

    #[test]
    fn rows_past_z_have_no_accelerator() {
        assert_eq!(accelerator_for(35), None);
    }

    #[test]
    fn accelerators_map_back_to_their_row() {
        for index in 0..35 {
            let key = accelerator_for(index).unwrap();
            assert_eq!(index_for_accelerator(key, 35), Some(index));
        }
    }

    #[test]
    fn an_accelerator_past_the_last_row_selects_nothing() {
        assert_eq!(index_for_accelerator('9', 3), None);
        assert_eq!(index_for_accelerator('a', 3), None);
    }

    #[test]
    fn uppercase_accelerators_still_match() {
        assert_eq!(index_for_accelerator('B', 12), Some(10));
    }

    #[test]
    fn non_accelerator_keys_select_nothing() {
        assert_eq!(index_for_accelerator('0', 20), None);
        assert_eq!(index_for_accelerator('!', 20), None);
    }

    #[test]
    fn row_text_is_shortened() {
        let spec = menu_item_spec(0, "abcdefghij", 7);
        assert_eq!(spec.text, "abcdef…");
        assert_eq!(spec.accelerator, Some('1'));
    }

    #[test]
    fn underscores_are_kept_verbatim() {
        // Nothing is parsed as a GTK mnemonic any more.
        assert_eq!(menu_item_spec(0, "foo_bar", 64).text, "foo_bar");
    }

    #[test]
    fn whitespace_is_collapsed_in_row_text() {
        assert_eq!(menu_item_spec(1, "a\n\tb   c", 64).text, "a b c");
    }

    #[test]
    fn a_point_on_the_second_output_is_localised() {
        let outputs = [
            OutputGeom::new(0, 0, 1920, 1080),
            OutputGeom::new(1920, 0, 800, 600),
        ];
        assert_eq!(
            position_on_outputs(2000, 40, &outputs, 0),
            Some((1, 80, 40))
        );
    }

    #[test]
    fn a_point_on_the_first_output_keeps_local_origin() {
        let outputs = [OutputGeom::new(0, 0, 1920, 1080)];
        assert_eq!(
            position_on_outputs(120, 80, &outputs, 0),
            Some((0, 120, 80))
        );
    }

    #[test]
    fn a_point_off_every_output_falls_back_to_primary() {
        let outputs = [
            OutputGeom::new(0, 0, 100, 100),
            OutputGeom::new(200, 0, 100, 100),
        ];
        assert_eq!(position_on_outputs(-10, -20, &outputs, 1), Some((1, 0, 0)));
    }

    #[test]
    fn no_outputs_means_no_position() {
        assert_eq!(position_on_outputs(0, 0, &[], 0), None);
    }

    fn output() -> [OutputGeom; 1] {
        [OutputGeom::new(0, 0, 1920, 1080)]
    }

    #[test]
    fn a_popup_with_room_below_stays_at_the_pointer() {
        assert_eq!(
            place_on_outputs(120, 80, 200, 300, &output(), 0),
            Some((0, 120, 80))
        );
    }

    #[test]
    fn a_popup_near_the_bottom_flips_above_the_pointer() {
        assert_eq!(
            place_on_outputs(120, 1000, 200, 200, &output(), 0),
            Some((0, 120, 800))
        );
    }

    #[test]
    fn a_popup_near_the_right_flips_left_of_the_pointer() {
        assert_eq!(
            place_on_outputs(1900, 80, 150, 100, &output(), 0),
            Some((0, 1750, 80))
        );
    }

    #[test]
    fn a_popup_larger_than_the_output_is_pinned_to_the_origin() {
        assert_eq!(
            place_on_outputs(400, 300, 2000, 1200, &output(), 0),
            Some((0, 0, 0))
        );
    }

    #[test]
    fn a_flipped_popup_that_still_overflows_slides_onto_the_output() {
        // Pointer near the top, menu taller than the space above or below:
        // flip goes off the top, then slide pins the box to y = 0.
        assert_eq!(
            place_on_outputs(10, 40, 100, 1060, &output(), 0),
            Some((0, 10, 0))
        );
    }

    #[test]
    fn a_popup_on_the_second_output_is_localised_then_placed() {
        let outputs = [
            OutputGeom::new(0, 0, 1920, 1080),
            OutputGeom::new(1920, 0, 800, 600),
        ];
        assert_eq!(
            place_on_outputs(2700, 500, 200, 200, &outputs, 0),
            Some((1, 580, 300))
        );
    }

    #[test]
    fn no_outputs_means_no_placement() {
        assert_eq!(place_on_outputs(0, 0, 10, 10, &[], 0), None);
    }
}
