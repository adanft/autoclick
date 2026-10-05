fn gray_mat(width: i32, height: i32) -> Mat {
    Mat::new_rows_cols_with_default(height, width, CV_8UC1, Scalar::all(0.0)).unwrap()
}

/// Hands out queued frames, recording how many were requested.
struct QueuedFrames {
    frames: VecDeque<Result<Mat>>,
    requests: usize,
}

impl QueuedFrames {
    fn new(frames: Vec<Result<Mat>>) -> Self {
        Self {
            frames: frames.into(),
            requests: 0,
        }
    }
}

impl FrameSource for QueuedFrames {
    fn capture_frame(&mut self) -> Result<Mat> {
        self.requests += 1;
        self.frames
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("no frame queued")))
    }
}

#[test]
fn captures_one_frame_per_call_and_keeps_its_pixels() {
    let mut frame = gray_mat(6, 4);
    *frame.at_2d_mut::<u8>(3, 5).unwrap() = 231;
    let source = QueuedFrames::new(vec![Ok(frame), Ok(gray_mat(2, 2))]);
    let mut capture = CaptureService::with_source("HDMI-A-1", source);

    let first = capture.capture_monitor().unwrap();
    let second = capture.capture_monitor().unwrap();

    assert_eq!(
        first.extent,
        crate::wayland_pointer::ImageExtent {
            width: 6,
            height: 4,
        }
    );
    assert_eq!(*first.image.at_2d::<u8>(3, 5).unwrap(), 231);
    assert_eq!(*first.image.at_2d::<u8>(0, 0).unwrap(), 0);
    assert_eq!(second.extent.width, 2);
    assert_eq!(capture.source.requests, 2);
}

#[test]
fn names_the_output_when_a_frame_fails() {
    let source = QueuedFrames::new(vec![Err(anyhow!("screencopy frame failed"))]);
    let mut capture = CaptureService::with_source("HDMI-A-1", source);

    let error = format!("{:#}", capture.capture_monitor().unwrap_err());

    assert!(error.contains("HDMI-A-1"), "unexpected error: {error}");
    assert!(error.contains("screencopy frame failed"), "unexpected error: {error}");
}

#[test]
fn rejects_a_frame_without_extent() {
    let source = QueuedFrames::new(vec![Ok(Mat::default())]);
    let mut capture = CaptureService::with_source("DP-1", source);

    let error = format!("{:#}", capture.capture_monitor().unwrap_err());

    assert!(error.contains("captured screenshot has no usable extent"));
}

#[test]
fn records_only_positive_decoded_capture_extents() {
    let image = CapturedImage::from_decoded(gray_mat(2560, 1440)).unwrap();

    assert_eq!(
        image.extent,
        crate::wayland_pointer::ImageExtent {
            width: 2560,
            height: 1440,
        }
    );
    assert!(CapturedImage::from_decoded(Mat::default()).is_err());
}
