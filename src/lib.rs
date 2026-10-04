use aviutl2::{anyhow, common::AviUtl2Info, image, input::*, lprintln};
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
use num_rational::Rational32;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

fn image_format_name(path: &Path) -> &'static str {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("heif"))
    {
        "HEIF"
    } else {
        "HEIC"
    }
}

fn decode_heic(path: &Path) -> anyhow::Result<image::RgbaImage> {
    let format = image_format_name(path);
    let bytes = fs::read(path).map_err(|error| anyhow::anyhow!("Failed to read file: {error}"))?;
    let heif = LibHeif::new();
    let context = HeifContext::read_from_bytes(&bytes)
        .map_err(|error| anyhow::anyhow!("Failed to parse {format} file: {error}"))?;
    let handle = context
        .primary_image_handle()
        .map_err(|error| anyhow::anyhow!("Failed to get primary image: {error}"))?;
    let decoded = heif
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgba), None)
        .map_err(|error| anyhow::anyhow!("Failed to decode {format} image: {error}"))?;

    let width = decoded.width();
    let height = decoded.height();
    anyhow::ensure!(width > 0 && height > 0, "HEIC image has an empty size");

    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("Image row size overflow"))?;
    let height_usize =
        usize::try_from(height).map_err(|_| anyhow::anyhow!("Image height is too large"))?;
    let output_len = row_bytes
        .checked_mul(height_usize)
        .ok_or_else(|| anyhow::anyhow!("Image buffer size overflow"))?;

    let planes = decoded.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| anyhow::anyhow!("Decoded image has no interleaved pixel plane"))?;
    anyhow::ensure!(
        plane.stride >= row_bytes,
        "Invalid HEIC row stride: {} (minimum {})",
        plane.stride,
        row_bytes
    );
    let source_len = plane
        .stride
        .checked_mul(height_usize)
        .ok_or_else(|| anyhow::anyhow!("Source image buffer size overflow"))?;
    anyhow::ensure!(
        plane.data.len() >= source_len,
        "Decoded HEIC pixel plane is truncated"
    );

    let mut pixels = Vec::with_capacity(output_len);
    for row in plane.data.chunks_exact(plane.stride).take(height_usize) {
        pixels.extend_from_slice(&row[..row_bytes]);
    }
    anyhow::ensure!(
        pixels.len() == output_len,
        "Decoded HEIC pixel size mismatch"
    );
    lprintln!("[{}] decoded: {}x{}", format, width, height);

    image::RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| anyhow::anyhow!("RgbaImage::from_raw failed"))
}

#[aviutl2::plugin(InputPlugin)]
struct HeicInputPlugin {
    cache: Mutex<HashMap<PathBuf, Arc<image::RgbaImage>>>,
}

impl InputPlugin for HeicInputPlugin {
    type InputHandle = Arc<image::RgbaImage>;

    fn new(_info: AviUtl2Info) -> anyhow::Result<Self> {
        Ok(Self {
            cache: Mutex::new(HashMap::new()),
        })
    }

    fn plugin_info(&self) -> InputPluginTable {
        InputPluginTable {
            name: "HEIF_HEIC Input Plugin".to_string(),
            information: "HEIC/HEIF Image Input Plugin".to_string(),
            input_type: InputType::Video,
            concurrent: false,
            file_filters: vec![FileFilter {
                name: "HEIC/HEIF Image".to_string(),
                extensions: vec!["heic".to_string(), "heif".to_string()],
            }],
            can_config: false,
        }
    }

    fn open(&self, file: PathBuf) -> anyhow::Result<Arc<image::RgbaImage>> {
        let format = image_format_name(&file);
        lprintln!("[{}] open: {:?}", format, file);
        let mut cache = self.cache.lock().unwrap();
        if let Some(image) = cache.get(&file) {
            return Ok(Arc::clone(image));
        }

        let image = match decode_heic(&file) {
            Ok(image) => Arc::new(image),
            Err(error) => {
                lprintln!("[{}] decode failed: {:#}", format, error);
                return Err(error);
            }
        };
        cache.insert(file, Arc::clone(&image));
        Ok(image)
    }

    fn close(&self, _handle: Arc<image::RgbaImage>) -> anyhow::Result<()> {
        Ok(())
    }

    fn get_input_info(
        &self,
        handle: &mut Arc<image::RgbaImage>,
        _video_track: u32,
        _audio_track: u32,
    ) -> anyhow::Result<InputInfo> {
        let (width, height) = handle.dimensions();
        Ok(InputInfo {
            video: Some(VideoInputInfo {
                fps: Rational32::new(1, 1),
                num_frames: 1,
                manual_frame_index: false,
                width,
                height,
                format: InputPixelFormat::Bgra,
            }),
            audio: None,
        })
    }

    fn read_video_mut(
        &self,
        handle: &mut Arc<image::RgbaImage>,
        frame: u32,
        returner: &mut ImageReturner,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(frame == 0, "Only one frame available (got frame={})", frame);
        returner.write(handle.as_ref());
        Ok(())
    }
}

aviutl2::register_input_plugin!(HeicInputPlugin);
