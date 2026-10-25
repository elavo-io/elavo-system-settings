//! Account pictures. AccountsService refuses an icon file over 1 MB, so a
//! photo is cropped to a square and shrunk with ImageMagick before it is
//! stored. Monograms are drawn by GTK in `panes::picture`. Suggestions are the pictures
//! in `elavo-visuals` `assets/user-images`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// AccountsService's limit in `user_change_icon_file_authorized_cb`.
pub const ICON_MAX_BYTES: u64 = 1_048_576;
/// The square edge we store. 512px JPEG or PNG stays far under the limit.
pub const ICON_EDGE: u32 = 512;

/// One monogram background. `top` and `bottom` are a vertical gradient;
/// equal values are the flat gray Settings already uses when there is no
/// picture. The letter is white, or near-black when the middle of the
/// gradient is light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tone {
    pub name: &'static str,
    pub top: &'static str,
    pub bottom: &'static str,
}

const fn tone(name: &'static str, top: &'static str, bottom: &'static str) -> Tone {
    Tone { name, top, bottom }
}

/// The first tone is the flat gray circle the users list draws today.
pub const TONES: &[Tone] = &[
    tone("Gray", "#8e8e93", "#8e8e93"),
    tone("Blue", "#6aa8ff", "#0860f2"),
    tone("Purple", "#c9a6ff", "#6b4cdb"),
    tone("Pink", "#ff9ec4", "#d6337f"),
    tone("Red", "#ff9a94", "#d93b36"),
    tone("Orange", "#ffb56a", "#c95a06"),
    tone("Yellow", "#ffe08a", "#f2c230"),
    tone("Green", "#8ee0a8", "#258a3b"),
    tone("Teal", "#7ee0d6", "#0e8f86"),
    tone("Graphite", "#c8c8cc", "#5c5c60"),
];

/// Where a picture sits in the square crop view, in view pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub scale: f32,
    pub ox: f32,
    pub oy: f32,
}

/// The square of source pixels that fills the crop view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub size: u32,
}

/// Cover scale, then clamp the offset so the square stays filled.
pub fn place(tex_w: f32, tex_h: f32, view: f32, zoom: f32, ox: f32, oy: f32) -> Placement {
    let zoom = zoom.clamp(1.0, 3.0);
    let scale = if tex_w <= 0.0 || tex_h <= 0.0 || view <= 0.0 {
        1.0
    } else {
        zoom * (view / tex_w).max(view / tex_h)
    };
    let (cw, ch) = (tex_w * scale, tex_h * scale);
    Placement {
        scale,
        ox: ox.clamp(view - cw, 0.0),
        oy: oy.clamp(view - ch, 0.0),
    }
}

/// Offset that centers the picture at `zoom`.
pub fn centered(tex_w: f32, tex_h: f32, view: f32, zoom: f32) -> Placement {
    let scale = place(tex_w, tex_h, view, zoom, 0.0, 0.0).scale;
    place(
        tex_w,
        tex_h,
        view,
        zoom,
        (view - tex_w * scale) / 2.0,
        (view - tex_h * scale) / 2.0,
    )
}

/// Keep the source pixel under the middle of the view when the zoom changes.
pub fn zoom_about_center(
    tex_w: f32,
    tex_h: f32,
    view: f32,
    current: Placement,
    zoom: f32,
) -> Placement {
    let (cx, cy) = (view / 2.0, view / 2.0);
    let src_x = (cx - current.ox) / current.scale;
    let src_y = (cy - current.oy) / current.scale;
    let next = place(tex_w, tex_h, view, zoom, 0.0, 0.0);
    place(
        tex_w,
        tex_h,
        view,
        zoom,
        cx - src_x * next.scale,
        cy - src_y * next.scale,
    )
}

pub fn crop_square(tex_w: f32, tex_h: f32, view: f32, place: Placement) -> Crop {
    if tex_w <= 1.0 || tex_h <= 1.0 || place.scale <= 0.0 {
        return Crop {
            x: 0,
            y: 0,
            size: 1,
        };
    }
    let mut x = ((-place.ox) / place.scale).max(0.0);
    let mut y = ((-place.oy) / place.scale).max(0.0);
    let mut size = view / place.scale;
    if x + size > tex_w {
        size = tex_w - x;
    }
    if y + size > tex_h {
        size = tex_h - y;
    }
    if x + size > tex_w {
        x = (tex_w - size).max(0.0);
    }
    if y + size > tex_h {
        y = (tex_h - size).max(0.0);
    }
    let size = size.round().max(1.0).min(tex_w.min(tex_h));
    Crop {
        x: x.round().min(tex_w - size).max(0.0) as u32,
        y: y.round().min(tex_h - size).max(0.0) as u32,
        size: size as u32,
    }
}

fn linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn parse_hex(hex: &str) -> Result<[u8; 3], ()> {
    let hex = hex.strip_prefix('#').ok_or(())?;
    if hex.len() != 6 {
        return Err(());
    }
    let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| ())?;
    let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| ())?;
    let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| ())?;
    Ok([r, g, b])
}

/// Letter color for a tone, from the gradient's center. Light centers get
/// near-black; the rest get white.
pub fn letter_color(tone: Tone) -> &'static str {
    let (Ok(top), Ok(bottom)) = (parse_hex(tone.top), parse_hex(tone.bottom)) else {
        return "#ffffff";
    };
    let mid = [
        top[0] / 2 + bottom[0] / 2,
        top[1] / 2 + bottom[1] / 2,
        top[2] / 2 + bottom[2] / 2,
    ];
    let lum = 0.2126 * linear(mid[0]) + 0.7152 * linear(mid[1]) + 0.0722 * linear(mid[2]);
    if lum > 0.5 {
        "#1c1c1c"
    } else {
        "#ffffff"
    }
}

/// Capture nodes only. A webcam also exposes a metadata node at index 1.
pub fn is_capture_index(index: &str) -> bool {
    index.trim() == "0"
}

pub fn capture_devices() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return Vec::new();
    };
    let mut devices: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            let index = std::fs::read_to_string(entry.path().join("index")).ok()?;
            if !is_capture_index(&index) {
                return None;
            }
            let dev = PathBuf::from("/dev").join(entry.file_name());
            dev.exists().then_some(dev)
        })
        .collect();
    devices.sort();
    devices
}

/// Installed pictures first, then the visuals checkout on a development machine.
pub fn suggestion_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/usr/share/elavo/user-images")];
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("GitHub/elavo-visuals/assets/user-images"));
    }
    roots
}

pub fn suggestions(roots: &[PathBuf]) -> Vec<PathBuf> {
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| is_image(path))
            .collect();
        if files.is_empty() {
            continue;
        }
        files.sort();
        return files;
    }
    Vec::new()
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "webp"
            )
        })
        .unwrap_or(false)
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("ImageMagick is not available ({error})"))?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        Err(if detail.is_empty() {
            "ImageMagick could not prepare the picture.".to_owned()
        } else {
            detail.to_owned()
        })
    }
}

/// `-resize` geometry that shrinks to `edge` and never enlarges.
fn fit(edge: u32) -> String {
    format!("{edge}x{edge}>")
}

/// The longest edge of the copy the crop view shows. Large enough for a
/// sharp `ICON_EDGE` crop at the highest zoom, small enough to load quickly.
pub const WORKING_EDGE: u32 = 2048;

/// Turn any picture ImageMagick reads into an upright JPEG the crop view can
/// show. Phone photos are stored sideways with an EXIF rotation, which GTK
/// does not apply, so the copy is rotated here. Only the first frame of an
/// animation or a multi-page file is used, and transparency becomes white.
pub fn normalize(input: &Path, output: &Path) -> Result<(), String> {
    let mut source = input.as_os_str().to_owned();
    source.push("[0]");
    run(Command::new("magick")
        .arg(source)
        .args([
            "-auto-orient",
            "-background",
            "white",
            "-flatten",
            "-resize",
            &fit(WORKING_EDGE),
            "-quality",
            "92",
        ])
        .arg(output))
}

/// Crop to the square the person framed, then fit it in `ICON_EDGE`.
pub fn render_crop(input: &Path, crop: Crop, output: &Path) -> Result<(), String> {
    run(Command::new("magick")
        .arg(input)
        .arg("-crop")
        .arg(format!("{}x{}+{}+{}", crop.size, crop.size, crop.x, crop.y))
        .args(["+repage", "-resize", &fit(ICON_EDGE), "-quality", "85"])
        .arg(output))?;
    shrink_if_needed(output)
}

/// Shrink a picture that is already square, such as a camera frame.
pub fn render_square(input: &Path, output: &Path) -> Result<(), String> {
    run(Command::new("magick")
        .arg(input)
        .args(["-auto-orient", "-resize", &fit(ICON_EDGE), "-quality", "85"])
        .arg(output))?;
    shrink_if_needed(output)
}

fn shrink_if_needed(output: &Path) -> Result<(), String> {
    let len = std::fs::metadata(output)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if len <= ICON_MAX_BYTES {
        return Ok(());
    }
    let smaller = output.with_extension("small.jpg");
    run(Command::new("magick")
        .arg(output)
        .args(["-resize", &fit(ICON_EDGE), "-quality", "60"])
        .arg(&smaller))?;
    std::fs::rename(&smaller, output).map_err(|error| error.to_string())?;
    let len = std::fs::metadata(output)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if len > ICON_MAX_BYTES {
        Err("The picture is still too large to store.".into())
    } else {
        Ok(())
    }
}

/// The next JPEG in an ffmpeg image pipe. Bytes before the start marker are skipped.
pub fn next_jpeg(reader: &mut impl std::io::BufRead) -> std::io::Result<Vec<u8>> {
    let mut byte = [0];
    loop {
        reader.read_exact(&mut byte)?;
        if byte[0] != 0xff {
            continue;
        }
        reader.read_exact(&mut byte)?;
        if byte[0] == 0xd8 {
            break;
        }
    }
    let mut frame = vec![0xff, 0xd8];
    loop {
        reader.read_exact(&mut byte)?;
        frame.push(byte[0]);
        // A marker may be preceded by any number of 0xff fill bytes.
        while byte[0] == 0xff {
            reader.read_exact(&mut byte)?;
            frame.push(byte[0]);
            if byte[0] == 0xd9 {
                return Ok(frame);
            }
        }
    }
}

/// A suggestion is always copied out of `/usr/share` first. AccountsService
/// keeps a reference instead of a copy for files under `/usr/share`, and the
/// picture would break if an image update renamed or removed the file.
pub fn prepare_suggestion(path: &Path, output: &Path) -> Result<PathBuf, String> {
    let len = std::fs::metadata(path)
        .map(|meta| meta.len())
        .unwrap_or(u64::MAX);
    if len <= ICON_MAX_BYTES {
        std::fs::copy(path, output).map_err(|error| error.to_string())?;
    } else {
        render_square(path, output)?;
    }
    Ok(output.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_picture_at_normal_zoom_uses_the_whole_file() {
        let placed = centered(400.0, 400.0, 200.0, 1.0);
        let crop = crop_square(400.0, 400.0, 200.0, placed);
        assert_eq!(
            crop,
            Crop {
                x: 0,
                y: 0,
                size: 400
            }
        );
    }

    #[test]
    fn wide_picture_crops_the_sides() {
        let placed = centered(400.0, 200.0, 100.0, 1.0);
        let crop = crop_square(400.0, 200.0, 100.0, placed);
        assert_eq!(crop.size, 200);
        assert_eq!(crop.y, 0);
        assert_eq!(crop.x, 100);
    }

    #[test]
    fn zooming_in_keeps_the_center_and_shrinks_the_crop() {
        let start = centered(400.0, 200.0, 100.0, 1.0);
        let closer = zoom_about_center(400.0, 200.0, 100.0, start, 2.0);
        let crop = crop_square(400.0, 200.0, 100.0, closer);
        assert_eq!(crop.size, 100);
        assert_eq!(crop.x, 150);
        assert_eq!(crop.y, 50);
    }

    #[test]
    fn a_drag_cannot_uncover_the_square() {
        let placed = place(400.0, 200.0, 100.0, 1.0, 10_000.0, -10_000.0);
        assert!(placed.ox <= 0.0);
        assert!(placed.oy <= 0.0);
        let crop = crop_square(400.0, 200.0, 100.0, placed);
        assert!(crop.x + crop.size <= 400);
        assert!(crop.y + crop.size <= 200);
    }

    #[test]
    fn only_yellow_uses_a_dark_letter() {
        let dark: Vec<_> = TONES
            .iter()
            .copied()
            .filter(|tone| letter_color(*tone) == "#1c1c1c")
            .collect();
        assert_eq!(dark.len(), 1);
        assert_eq!(dark[0].top, "#ffe08a");
        assert_eq!(letter_color(TONES[0]), "#ffffff");
    }

    #[test]
    fn metadata_nodes_are_not_cameras() {
        assert!(is_capture_index("0"));
        assert!(is_capture_index(" 0\n"));
        assert!(!is_capture_index("1"));
        assert!(!is_capture_index(""));
    }

    #[test]
    fn suggestions_come_from_the_first_directory_with_pictures() {
        let root = std::env::temp_dir().join(format!("elavo-pictures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let empty = root.join("empty");
        let full = root.join("full");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&full).unwrap();
        std::fs::write(full.join("b.jpg"), b"b").unwrap();
        std::fs::write(full.join("a.jpg"), b"a").unwrap();
        std::fs::write(full.join("notes.txt"), b"no").unwrap();
        let found = suggestions(&[empty, full]);
        assert_eq!(found.len(), 2);
        assert!(found[0].ends_with("a.jpg"));
        assert!(found[1].ends_with("b.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn jpeg_frames_are_split_on_their_markers() {
        let bytes = vec![
            0, 1, 0xff, 0x00, 0xff, 0xd8, 0x11, 0xff, 0xd9, 0xff, 0xd8, 0xff, 0xd9,
        ];
        let mut reader = std::io::Cursor::new(&bytes);
        assert_eq!(
            next_jpeg(&mut reader).unwrap(),
            vec![0xff, 0xd8, 0x11, 0xff, 0xd9]
        );
        assert_eq!(
            next_jpeg(&mut reader).unwrap(),
            vec![0xff, 0xd8, 0xff, 0xd9]
        );
        assert!(next_jpeg(&mut reader).is_err());
    }

    #[test]
    fn fill_bytes_before_the_end_marker_still_end_the_frame() {
        let bytes = vec![0xff, 0xd8, 0x11, 0xff, 0xff, 0xd9, 0x22];
        let mut reader = std::io::Cursor::new(&bytes);
        assert_eq!(
            next_jpeg(&mut reader).unwrap(),
            vec![0xff, 0xd8, 0x11, 0xff, 0xff, 0xd9]
        );
    }
}
