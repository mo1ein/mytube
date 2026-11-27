use anyhow::{Result, ensure};
use objc2::{
    class, msg_send,
    rc::{Allocated, Retained, autoreleasepool},
    runtime::{AnyObject, Bool},
};
use objc2_foundation::{NSArray, NSDictionary, NSString, NSURL};
use std::path::Path;

#[link(name = "Vision", kind = "framework")]
unsafe extern "C" {}

// The selectors below are the synchronous Vision text recognition API. Keep
// all Objective-C objects on the worker and return owned Rust text to the UI.
pub(super) fn text(path: &Path) -> Result<String> {
    autoreleasepool(|_| unsafe {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let options = NSDictionary::<NSString, AnyObject>::new();
        let alloc: Allocated<AnyObject> = msg_send![class!(VNImageRequestHandler), alloc];
        let handler: Retained<AnyObject> = msg_send![alloc, initWithURL: &*url, options: &*options];
        let request: Retained<AnyObject> = msg_send![class!(VNRecognizeTextRequest), new];
        let requests = NSArray::from_slice(&[&*request]);
        let ok: Bool = msg_send![&*handler, performRequests: &*requests, error: std::ptr::null_mut::<*mut AnyObject>()];
        ensure!(ok.as_bool(), "Vision could not recognize this frame");
        let observations: Option<Retained<NSArray<AnyObject>>> = msg_send![&*request, results];
        let mut lines = Vec::new();
        if let Some(observations) = observations {
            for i in 0..observations.count() {
                let observation = observations.objectAtIndex(i);
                let candidates: Retained<NSArray<AnyObject>> =
                    msg_send![&*observation, topCandidates: 1usize];
                if let Some(candidate) = candidates.firstObject() {
                    let value: Retained<NSString> = msg_send![&*candidate, string];
                    lines.push(value.to_string());
                }
            }
        }
        Ok(lines.join("\n"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires macOS Vision and ImageMagick to generate the text fixture"]
    fn recognizes_words_from_a_video_frame() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let frame = dir.path().join("text.png");
        let status = std::process::Command::new("magick")
            .args([
                "-size",
                "640x160",
                "xc:white",
                "-font",
                "assets/Roboto-Regular.ttf",
                "-pointsize",
                "50",
                "-fill",
                "black",
                "-gravity",
                "center",
                "-annotate",
                "0",
                "MyTube video",
            ])
            .arg(&frame)
            .status()?;
        ensure!(status.success(), "Could not create the text fixture");
        let recognized = text(&frame)?;
        assert!(
            recognized.to_lowercase().contains("mytube video"),
            "{recognized}"
        );
        Ok(())
    }
}
