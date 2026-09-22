p = 'crates/gb-core/src/cartridge.rs'
s = open(p, encoding='utf-8').read()


def sub(old, new):
    global s
    assert old in s, "NOT FOUND:\n" + old[:200]
    assert s.count(old) == 1, "AMBIGUOUS: " + old[:60]
    s = s.replace(old, new, 1)


sub("""    /// What the lens is pointed at: `CAMERA_W * CAMERA_H` greyscale bytes, 0 is
    /// black. `None` means nothing is feeding us light, and captures develop a
    /// self-describing test card instead. See `camera_develop`.
    frame: Option<Vec<u8>>,""",
"""    /// What the lens is pointed at: `CAMERA_W * CAMERA_H` greyscale bytes, 0 is
    /// black. `None` means nothing is feeding us light, and captures develop a
    /// self-describing test card instead. See `camera_develop`.
    frame: Option<Vec<u8>>,
    /// Does the frontend have a camera at all?
    ///
    /// This exists to tell two failures apart that otherwise look identical: a
    /// frontend that never implemented the camera interface, and one that has a
    /// camera the player has not pointed at anything yet (or has refused
    /// permission for). Both leave `frame` as `None`, and without this the
    /// player and the developer see the same picture for a bug and for a
    /// prompt.
    sensor_available: bool,""")

sub("""            busy: 0,
            frame: None,
        }""",
"""            busy: 0,
            frame: None,
            sensor_available: false,
        }""")

# ---- two diagnostics instead of one ----------------------------------------
sub("""/// What the sensor sees when nothing is feeding it light.
///
/// This is a **diagnostic**, not a placeholder, and the difference matters. A
/// frontend that has not implemented the camera interface leaves the core with
/// no frames, and that failure looks exactly like a broken sensor model: the
/// picture is wrong and the cause is in somebody else's repository. So the
/// no-signal image is deliberately unmistakable rather than plausible.
///
/// Four vertical bars stepping through the four shades, cut by a diagonal.
/// Nothing a lens could produce, so nobody mistakes it for a photograph, and it
/// still exercises every shade and the whole dither matrix.
fn test_card(x: usize, y: usize) -> u8 {
    let bar = (x * 4 / CAMERA_W).min(3);
    let level = [30u8, 100, 170, 240][bar];
    if (x + y) % 32 < 3 {
        255 - level
    } else {
        level
    }
}""",
"""/// What the sensor sees when nothing is feeding it light.
///
/// These are **diagnostics**, not placeholders, and there are two of them on
/// purpose. A frontend with no camera interface and a frontend whose camera has
/// not produced a frame yet both leave the core with nothing, and those need
/// completely different responses: one is a bug to file against the frontend,
/// the other is a permission prompt to put in front of the player. A single
/// image for both means whoever sees it cannot tell which they are looking at,
/// which defeats the point of having a diagnostic at all.
///
/// Both are deliberately unmistakable rather than plausible: nothing a lens
/// could produce, so neither is ever mistaken for a photograph, and both
/// exercise all four shades and the whole dither matrix.
///
/// `NO CAMERA HERE`: four vertical bars stepping through the shades, cut by a
/// diagonal. Hard-edged and static.
///
/// `WAITING FOR LIGHT`: concentric rings. Round rather than straight, so the
/// two are distinguishable at a glance and across a blurry photograph of a
/// screen, which is how these are usually reported.
fn no_camera_card(x: usize, y: usize) -> u8 {
    let bar = (x * 4 / CAMERA_W).min(3);
    let level = [30u8, 100, 170, 240][bar];
    if (x + y) % 32 < 3 {
        255 - level
    } else {
        level
    }
}

fn waiting_for_light_card(x: usize, y: usize) -> u8 {
    let (dx, dy) = (
        x as i32 - CAMERA_W as i32 / 2,
        y as i32 - CAMERA_H as i32 / 2,
    );
    // Integer distance; no floating point in the core's hot paths.
    let r = ((dx * dx + dy * dy) as f32).sqrt() as usize;
    [240u8, 170, 100, 30][(r / 9) % 4]
}""")

sub("""                    let raw = match &cam.frame {
                        Some(f) => f[y * CAMERA_W + x],
                        None => test_card(x, y),
                    } as u32;""",
"""                    let raw = match (&cam.frame, cam.sensor_available) {
                        (Some(f), _) => f[y * CAMERA_W + x],
                        (None, true) => waiting_for_light_card(x, y),
                        (None, false) => no_camera_card(x, y),
                    } as u32;""")

# The indentation above differs; try the un-indented form too.
if "let raw = match (&cam.frame, cam.sensor_available)" not in s:
    sub("""                let raw = match &cam.frame {
                    Some(f) => f[y * CAMERA_W + x],
                    None => test_card(x, y),
                } as u32;""",
"""                let raw = match (&cam.frame, cam.sensor_available) {
                    (Some(f), _) => f[y * CAMERA_W + x],
                    (None, true) => waiting_for_light_card(x, y),
                    (None, false) => no_camera_card(x, y),
                } as u32;""")

sub("""    /// Whether this cartridge has an image sensor on it at all.
    pub fn has_camera(&self) -> bool {""",
"""    /// Tell the core whether the frontend has a camera at all.
    ///
    /// Call it with true once a camera interface has been obtained, whether or
    /// not any frame has arrived yet. It only chooses which diagnostic the
    /// sensor sees while no frame is available, and it is worth setting because
    /// "this frontend cannot do cameras" and "no picture has arrived yet" want
    /// different responses from whoever is looking at the screen.
    pub fn set_camera_available(&mut self, available: bool) {
        if let Mbc::Camera { cam, .. } = &mut self.mbc {
            cam.sensor_available = available;
        }
    }

    /// Whether this cartridge has an image sensor on it at all.
    pub fn has_camera(&self) -> bool {""")

open(p, 'w', encoding='utf-8', newline='\n').write(s)
print("two-state diagnostic written")
