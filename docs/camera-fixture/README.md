# Game Boy Camera capture fixture

A fixed input and the exact output it produces, so a frontend can check its
capture path against something other than itself.

The point is that every check a frontend can run on its own tells it the
pipeline is **self-consistent**, not that it is **correct**. These files are the
external reference.

```
scene-source.png      640x480, 4:3      what a phone sensor might hand you
scene-sensor.png      128x112, grey     what the core expects after your crop
scene-sensor.bin      14336 bytes       the same frame, exactly as the API takes it
scene-viewfinder.png  160x144           the Game Boy screen that produces
```

## How to use it

**Check your crop and downscale.** Feed `scene-source.png` through whatever your
capture path does, and compare the result with `scene-sensor.png`.

Small differences are fine and expected: the reference was produced with a
centre crop to 8:7 and nearest-neighbour downscaling, and a bilinear resampler
will differ slightly. What must match is the *content*, and the source is built
so that getting it wrong is unmissable rather than subtle:

- **The "F"** is asymmetric in both axes. Mirrored, it is obviously mirrored.
  Rotated or flipped, obviously so. This is the one that matters most: a
  mirrored frame develops text backwards, and it does so in a **print**, which
  is the artifact somebody keeps.
- **The four corner pips** sit just inside the 8:7 crop. If all four are present
  and roughly equidistant from the edges, the crop is centred and correct.
- **Everything outside the crop is dim.** A frontend that crops too wide lets a
  dark border into the picture, which is impossible to miss.

**Check the core end to end.** Feed `scene-sensor.bin` to the core and you must
get `scene-viewfinder.png` **byte for byte**. The `.bin` is the authoritative
copy: it is 14336 raw bytes, exactly what `set_camera_frame` takes, with nothing
to decode and so nothing to get wrong between the fixture and the contract. The
`.png` is the same data, for looking at. The output is deterministic;
feeding the same frame twice produces an identical screen. There is a test in
`crates/gb-core/tests/camera.rs` that asserts exactly this, so if it ever stops
being true, that is a regression rather than a tolerance.

```sh
GBCAMERA=docs/camera-fixture/scene-sensor.png \
cargo run --release -p gb-runner --bin shot -- \
    "dumps/roms/Game Boy Camera (USA, Europe) (SGB Enhanced).gb" \
    40 out.png "w900,a,w180,a,w180,a,w180"
```

## The buffer contract

`gb_core::set_camera_frame` takes `CAMERA_W * CAMERA_H` = 128 * 112 = **14336
bytes**, one per pixel.

**0 is black.** Measured rather than asserted: feeding all-zeros gives a
viewfinder with a mean luminance of 21, and all-255 gives 142. An inverted ramp
is exactly the sort of bug that looks like an artistic choice, so it is worth
checking against this rather than against taste.

Frames arrive **unmirrored, in sensor orientation**. Mirror your preview if your
users expect to aim like a mirror; hand the core the unmirrored frame.

The **frontend** crops to 8:7, not the core, because the crop has to match the
preview the player is aiming with. A core cropping differently would make the
viewfinder lie about what the photograph will contain.
