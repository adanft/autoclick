    use opencv::core::CV_8UC1;
    use opencv::prelude::MatTraitConst;

    /// BT.601 luma OpenCV produces for saturated primaries and white.
    const RED_GRAY: u8 = 76;
    const GREEN_GRAY: u8 = 150;
    const BLUE_GRAY: u8 = 29;
    const WHITE_GRAY: u8 = 255;

    fn layout(format: Format, width: u32, height: u32, stride: u32) -> FrameLayout {
        FrameLayout {
            format,
            width,
            height,
            stride,
        }
    }

    /// Lays out rows of 4-byte pixels with `padding` trailing bytes per row.
    fn frame_bytes(rows: &[&[[u8; 4]]], padding: usize) -> Vec<u8> {
        rows.iter()
            .flat_map(|row| {
                row.iter()
                    .flatten()
                    .copied()
                    .chain(std::iter::repeat_n(0xAB, padding))
            })
            .collect()
    }

    fn gray_rows(mat: &Mat) -> Vec<Vec<u8>> {
        (0..mat.rows())
            .map(|row| {
                (0..mat.cols())
                    .map(|col| *mat.at_2d::<u8>(row, col).unwrap())
                    .collect()
            })
            .collect()
    }

    // Memory order on little-endian: Xrgb8888/Argb8888 are B,G,R,X and
    // Xbgr8888/Abgr8888 are R,G,B,X.
    const BGRX_RED: [u8; 4] = [0, 0, 255, 0];
    const BGRX_GREEN: [u8; 4] = [0, 255, 0, 0];
    const BGRX_BLUE: [u8; 4] = [255, 0, 0, 0];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    #[test]
    fn converts_xrgb_frames_as_bgra_memory_order() {
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 0);

        for format in [Format::Xrgb8888, Format::Argb8888] {
            let gray = shm_frame_to_grayscale(&pixels, &layout(format, 2, 2, 8), false).unwrap();

            assert_eq!(gray.typ(), CV_8UC1);
            assert_eq!(
                gray_rows(&gray),
                vec![vec![RED_GRAY, GREEN_GRAY], vec![BLUE_GRAY, WHITE_GRAY]]
            );
        }
    }

    #[test]
    fn converts_xbgr_frames_as_rgba_memory_order() {
        // The same bytes read as R,G,B,X swap red and blue.
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 0);

        for format in [Format::Xbgr8888, Format::Abgr8888] {
            let gray = shm_frame_to_grayscale(&pixels, &layout(format, 2, 2, 8), false).unwrap();

            assert_eq!(
                gray_rows(&gray),
                vec![vec![BLUE_GRAY, GREEN_GRAY], vec![RED_GRAY, WHITE_GRAY]]
            );
        }
    }

    #[test]
    fn skips_row_padding_beyond_the_visible_width() {
        let pixels = frame_bytes(
            &[&[BGRX_RED, BGRX_GREEN, BGRX_BLUE], &[WHITE, WHITE, BGRX_RED]],
            4,
        );

        let gray =
            shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 3, 2, 16), false).unwrap();

        assert_eq!((gray.cols(), gray.rows()), (3, 2));
        assert_eq!(
            gray_rows(&gray),
            vec![
                vec![RED_GRAY, GREEN_GRAY, BLUE_GRAY],
                vec![WHITE_GRAY, WHITE_GRAY, RED_GRAY]
            ]
        );
    }

    #[test]
    fn flips_vertically_when_the_frame_is_y_inverted() {
        let pixels = frame_bytes(&[&[BGRX_RED, BGRX_GREEN], &[BGRX_BLUE, WHITE]], 4);

        let gray =
            shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 2, 2, 12), true).unwrap();

        assert_eq!(
            gray_rows(&gray),
            vec![vec![BLUE_GRAY, WHITE_GRAY], vec![RED_GRAY, GREEN_GRAY]]
        );
    }

    #[test]
    fn rejects_unsupported_formats() {
        let pixels = vec![0; 8];

        let error = shm_frame_to_grayscale(&pixels, &layout(Format::Rgb565, 2, 1, 8), false)
            .unwrap_err();

        assert!(
            error.to_string().contains("unsupported wl_shm format"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rejects_empty_or_oversized_dimensions() {
        for (width, height, stride) in [(0, 2, 8), (2, 0, 8), (u32::MAX, 1, u32::MAX)] {
            let error = shm_frame_to_grayscale(
                &[],
                &layout(Format::Xrgb8888, width, height, stride),
                false,
            )
            .unwrap_err();

            assert!(
                error.to_string().contains("frame extent"),
                "unexpected error for {width}x{height}: {error}"
            );
        }
    }

    #[test]
    fn rejects_strides_that_cannot_hold_a_row_of_whole_pixels() {
        for stride in [4, 10] {
            let pixels = vec![0; stride as usize * 2];

            let error =
                shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 2, 2, stride), false)
                    .unwrap_err();

            assert!(
                error.to_string().contains("stride"),
                "unexpected error for stride {stride}: {error}"
            );
        }
    }

    #[test]
    fn rejects_pixel_data_that_does_not_match_stride_times_height() {
        for length in [15, 17] {
            let pixels = vec![0; length];

            let error = shm_frame_to_grayscale(&pixels, &layout(Format::Xrgb8888, 2, 2, 8), false)
                .unwrap_err();

            assert!(
                error.to_string().contains("expected 16 bytes"),
                "unexpected error for {length} bytes: {error}"
            );
        }
    }
