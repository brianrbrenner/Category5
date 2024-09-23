/// Thundr tests
///
/// Austin Shafer - 2024
use crate as th;

/// our generic pixel result checker
///
/// In this case we simply hash the raw pixel dump and compare
/// it against the known gold image for the test.
///
/// We can't directly check the pixel values, or hash the results.
/// Hashing might use different algorithms on other rust versions
/// meaning it may mismatch for no reason on a machine. We use
/// the perceptualdiff program to compare with the gold image as
/// different vendors may subltly round float values differently,
/// causing a mismatch. Perceptualdiff compares the two images
/// adjusting for perceivable errors, returning 0 if there are none.
fn check_pixels(display: &mut th::Display, filename: &str) {
    display.dump_framebuffer(filename);
    let goldfile = ["golds/", filename].join("");

    assert!(std::process::Command::new("perceptualdiff")
        .arg(filename)
        .arg(goldfile)
        .status()
        .expect("perceptualdiff error, probable mismatch")
        .success());
}

/// Initialize our thundr test
fn init_thundr() -> (th::Thundr, th::Display) {
    let info = th::CreateInfo::builder()
        .surface_type(th::SurfaceType::Headless)
        .build();

    let mut thund = th::Thundr::new(&info).unwrap();
    let display = thund.get_display(&info).unwrap();

    (thund, display)
}

#[test]
fn basic_image() {
    let (mut thund, mut display) = init_thundr();
    let res = display.get_resolution();
    let viewport = th::Viewport::new(0, 0, res.0 as i32, res.1 as i32);

    // ------------ init an image -------------
    let size = 64;
    let u_size = size as usize;
    let pixels: Vec<u8> = std::iter::repeat(128).take(4 * u_size * u_size).collect();
    // Create an image from our MemImage
    let image = thund
        .create_image_from_bits(
            pixels.as_slice(),
            size, // width of texture
            size, // height of texture
            size, // stride
            None,
        )
        .unwrap();
    // Now create a 16x16 surface at position (0, 0)
    let surf = th::Surface::new(th::Rect::new(0, 0, 16, 16), Some(image), None);

    // ------------ draw a frame -------------
    display.begin_recording().unwrap();
    display.set_viewport(&viewport).unwrap();
    display.draw_surface(&surf).unwrap();
    display.end_recording().unwrap();

    display.present().unwrap();

    // ------------ check output -------------
    check_pixels(&mut display, "basic_image.ppm");
}

#[test]
fn basic_color() {
    let (_thund, mut display) = init_thundr();
    let res = display.get_resolution();
    let viewport = th::Viewport::new(0, 0, res.0 as i32, res.1 as i32);

    // Now create a 16x16 red square at position (32, 32)
    let surf = th::Surface::new(
        th::Rect::new(128, 128, 128, 128),
        None,
        Some((256.0, 0.0, 0.0, 1.0)),
    );

    // ------------ draw a frame -------------
    display.begin_recording().unwrap();
    display.set_viewport(&viewport).unwrap();
    display.draw_surface(&surf).unwrap();
    display.end_recording().unwrap();

    display.present().unwrap();

    // ------------ check output -------------
    check_pixels(&mut display, "basic_color.ppm");
}

#[test]
fn many_colors() {
    let (_thundr, mut display) = init_thundr();
    let res = display.get_resolution();
    let viewport = th::Viewport::new(0, 0, res.0 as i32, res.1 as i32);

    // ------------ draw a frame -------------
    display.begin_recording().unwrap();
    display.set_viewport(&viewport).unwrap();

    // Draw 100 overlapping colored squares
    for i in 0..10 {
        for j in 0..10 {
            let surf = th::Surface::new(
                th::Rect::new(128 + i * 20, 128 + j * 20, 16, 16),
                None,
                Some((
                    j as f32 / 10.0,
                    0.5 + (i as f32 * 0.02),
                    0.1 + (j as f32 * 0.03),
                    1.0,
                )),
            );
            display.draw_surface(&surf).unwrap();
        }
    }

    display.end_recording().unwrap();
    display.present().unwrap();

    // ------------ check output -------------
    check_pixels(&mut display, "many_colors.ppm");
}

#[test]
fn redraw() {
    let (mut thund, mut display) = init_thundr();
    let res = display.get_resolution();
    let viewport = th::Viewport::new(0, 0, res.0 as i32, res.1 as i32);

    // ------------ init an image -------------
    let size = 64;
    let u_size = size as usize;
    let pixels: Vec<u8> = std::iter::repeat(128).take(4 * u_size * u_size).collect();
    // Create an image from our MemImage
    let image = thund
        .create_image_from_bits(
            pixels.as_slice(),
            size, // width of texture
            size, // height of texture
            size, // stride
            None,
        )
        .unwrap();

    // ------------ draw a frame -------------
    display.begin_recording().unwrap();
    display.set_viewport(&viewport).unwrap();
    let surf = th::Surface::new(th::Rect::new(0, 0, 16, 16), Some(image.clone()), None);
    display.draw_surface(&surf).unwrap();
    display.end_recording().unwrap();

    display.present().unwrap();

    // ------------ draw a second frame -------------
    display.begin_recording().unwrap();
    display.set_viewport(&viewport).unwrap();
    let surf = th::Surface::new(th::Rect::new(32, 32, 16, 16), Some(image), None);
    display.draw_surface(&surf).unwrap();
    display.end_recording().unwrap();

    display.present().unwrap();

    // ------------ check output -------------
    check_pixels(&mut display, "redraw.ppm");
}
