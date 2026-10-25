use std::{
    fs::{self, File},
    io::Write,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Quality {
    Light,
    #[default]
    High,
    Original,
}

impl Quality {
    pub const ALL: [Self; 3] = [Self::Light, Self::High, Self::Original];

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Leve",
            Self::High => "Alta",
            Self::Original => "Original",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Light => "720p · 30 fps",
            Self::High => "1080p · 30 fps",
            Self::Original => "Nativa · 60 fps",
        }
    }

    pub fn fps(self) -> u64 {
        if self == Self::Original { 60 } else { 30 }
    }

    fn crf(self) -> &'static str {
        match self {
            Self::Light => "26",
            Self::High => "20",
            Self::Original => "17",
        }
    }

    pub fn dimensions(self, width: u32, height: u32) -> (u32, u32) {
        let max_height = match self {
            Self::Light => 720,
            Self::High => 1080,
            Self::Original => height,
        };
        let scale = (max_height as f64 / height as f64).min(1.0);
        // H.264 yuv420p requires even dimensions. Never upscale the source.
        let even = |n: u32| n.max(2) & !1;
        (
            even((width as f64 * scale) as u32),
            even((height as f64 * scale) as u32),
        )
    }
}

pub fn output_directory() -> PathBuf {
    dirs::video_dir()
        .or_else(dirs::document_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("DevClip")
}

fn ffmpeg_path() -> PathBuf {
    // A portable installation may keep FFmpeg beside the executable.
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let local = parent.join("ffmpeg.exe");
        if local.is_file() {
            return local;
        }
    }
    PathBuf::from("ffmpeg.exe")
}

pub struct Recording {
    pub paused: Arc<AtomicBool>,
    pub stopping: bool,
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    fps: u64,
    pub result: Receiver<Result<PathBuf, String>>,
}

impl Recording {
    pub fn start(quality: Quality) -> Self {
        let paused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let (tx, result) = mpsc::channel();
        let flags = (paused.clone(), stop.clone(), frames.clone());
        thread::spawn(move || {
            let result =
                record(quality, flags.0, flags.1, flags.2).map_err(|error| format!("{error:#}"));
            let _ = tx.send(result);
        });
        Self {
            paused,
            stopping: false,
            stop,
            frames,
            fps: quality.fps(),
            result,
        }
    }

    pub fn toggle_pause(&self) {
        self.paused.fetch_xor(true, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn has_frames(&self) -> bool {
        self.frames.load(Ordering::Relaxed) > 0
    }

    pub fn elapsed(&self) -> Duration {
        Duration::from_secs_f64(self.frames.load(Ordering::Relaxed) as f64 / self.fps as f64)
    }

    pub fn stop(&mut self) {
        self.stopping = true;
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Pixels {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

type LatestFrame = Arc<Mutex<Option<Arc<Pixels>>>>;

struct Capture {
    latest: LatestFrame,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Capture {
    type Flags = LatestFrame;
    type Error = anyhow::Error;

    fn new(ctx: Context<Self::Flags>) -> Result<Self> {
        Ok(Self {
            latest: ctx.flags,
            scratch: Vec::new(),
        })
    }

    fn on_frame_arrived(&mut self, frame: &mut Frame<'_>, _: InternalCaptureControl) -> Result<()> {
        let width = frame.width();
        let height = frame.height();
        let buffer = frame.buffer()?;
        let bytes = buffer.as_nopadding_buffer(&mut self.scratch).to_vec();
        *self
            .latest
            .lock()
            .map_err(|_| anyhow!("Falha ao acessar a captura"))? = Some(Arc::new(Pixels {
            width,
            height,
            bytes,
        }));
        Ok(())
    }
}

// Always reap the encoder on errors (disk full, broken pipe, display disconnect).
struct Encoder(Child);

impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn encoder(quality: Quality, frame: &Pixels, output: &Path, log: File) -> Result<Encoder> {
    let (width, height) = quality.dimensions(frame.width, frame.height);
    let child = Command::new(ffmpeg_path())
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-n",
            "-f",
            "rawvideo",
            "-pixel_format",
            "bgra",
        ])
        .args(["-video_size", &format!("{}x{}", frame.width, frame.height)])
        .args([
            "-framerate",
            &quality.fps().to_string(),
            "-i",
            "pipe:0",
            "-an",
        ])
        .args(["-vf", &format!("scale={width}:{height}:flags=bicubic")])
        .args([
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            quality.crf(),
        ])
        .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
        .arg(output)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context(
            "FFmpeg não encontrado. Instale-o no PATH ou coloque ffmpeg.exe ao lado do DevClip",
        )?;
    Ok(Encoder(child))
}

fn record(
    quality: Quality,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    count: Arc<AtomicU64>,
) -> Result<PathBuf> {
    let directory = output_directory();
    fs::create_dir_all(&directory).context("Não foi possível criar a pasta de vídeos")?;
    let output = directory.join(format!(
        "DevClip_{}.mp4",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S_%3f")
    ));
    let log_path = output.with_extension("log");
    let monitor = Monitor::primary().context("Monitor principal indisponível")?;
    let latest: LatestFrame = Arc::new(Mutex::new(None));
    let settings = Settings::new(
        monitor,
        CursorCaptureSettings::WithCursor,
        DrawBorderSettings::Default,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        latest.clone(),
    );
    let capture = Capture::start_free_threaded(settings)
        .context("Não foi possível iniciar a captura do Windows")?;

    // The writer has its own clock: WGC emits frames on desktop changes only.
    // Duplicating the latest frame preserves duration on a completely static desktop.
    let result = (|| -> Result<()> {
        let startup = Instant::now();
        let first = loop {
            if stop.load(Ordering::SeqCst) {
                bail!("Gravação cancelada antes do primeiro quadro");
            }
            if let Some(frame) = latest.lock().unwrap().clone() {
                break frame;
            }
            if capture.is_finished() || startup.elapsed() > Duration::from_secs(10) {
                bail!("O Windows não entregou quadros. Confira se a tela está desbloqueada");
            }
            thread::sleep(Duration::from_millis(10));
        };
        let log = File::create(&log_path)?;
        let mut encoder = encoder(quality, &first, &output, log)?;
        let mut input = encoder
            .0
            .stdin
            .take()
            .context("FFmpeg sem entrada de vídeo")?;
        let interval = Duration::from_secs_f64(1.0 / quality.fps() as f64);
        let mut next_frame = Instant::now();
        let mut was_paused = false;
        let mut lag = Duration::ZERO;

        while !stop.load(Ordering::SeqCst) {
            if capture.is_finished() {
                bail!("A captura foi interrompida pelo Windows");
            }
            if paused.load(Ordering::SeqCst) {
                was_paused = true;
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            if was_paused {
                next_frame = Instant::now();
                was_paused = false;
            }
            let now = Instant::now();
            if next_frame > now {
                thread::sleep(next_frame - now);
            }
            let frame = latest
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| first.clone());
            if (frame.width, frame.height) != (first.width, first.height) {
                bail!("A resolução da tela mudou durante a gravação. Inicie uma nova gravação");
            }
            input
                .write_all(&frame.bytes)
                .context("O codificador parou de receber vídeo")?;
            count.fetch_add(1, Ordering::Relaxed);
            next_frame += interval;
            // A short delay is recovered by the fixed clock. Sustained overload is
            // reported rather than silently producing a video in fast motion.
            let behind = Instant::now().saturating_duration_since(next_frame);
            lag = if behind > Duration::from_millis(500) {
                lag + interval
            } else {
                Duration::ZERO
            };
            if lag > Duration::from_secs(3) {
                bail!("O computador não acompanhou esta qualidade. Tente Leve ou Alta");
            }
        }
        drop(input); // EOF flushes H.264 and writes the MP4 index.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = encoder.0.try_wait()? {
                if !status.success() {
                    bail!(
                        "FFmpeg falhou: {}",
                        fs::read_to_string(&log_path).unwrap_or_default().trim()
                    );
                }
                break;
            }
            if Instant::now() > deadline {
                bail!(
                    "O codificador demorou demais para finalizar. Consulte {}",
                    log_path.display()
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    })();
    let capture_result = capture.stop();
    // Keep diagnostic logs on failure; don't claim a partial video as success.
    if let Err(error) = result {
        let details = fs::read_to_string(&log_path).unwrap_or_default();
        bail!(
            "{error:#}{}",
            if details.is_empty() {
                String::new()
            } else {
                format!("\n{}", details.trim())
            }
        );
    }
    capture_result.context("Falha ao encerrar a captura")?;
    let _ = fs::remove_file(log_path);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_dimensions_fit_source_and_keep_even_pixels() {
        assert_eq!(Quality::Light.dimensions(1920, 1080), (1280, 720));
        assert_eq!(Quality::High.dimensions(3840, 2160), (1920, 1080));
        assert_eq!(Quality::High.dimensions(1365, 767), (1364, 766));
        assert_eq!(Quality::Original.dimensions(2560, 1440), (2560, 1440));
        assert_eq!(Quality::Light.dimensions(1080, 1920), (404, 720));
    }

    #[test]
    #[ignore = "grava a tela local; requer sessão gráfica desbloqueada e FFmpeg"]
    fn captures_mp4_and_omits_paused_time() {
        let mut recording = Recording::start(Quality::Light);
        let startup = Instant::now();
        while !recording.has_frames() {
            if let Ok(result) = recording.result.try_recv() {
                panic!("Captura encerrou antes de iniciar: {result:?}");
            }
            assert!(
                startup.elapsed() < Duration::from_secs(15),
                "Captura não iniciou"
            );
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_secs(1));
        recording.toggle_pause();
        thread::sleep(Duration::from_millis(100));
        let before = recording.elapsed();
        thread::sleep(Duration::from_secs(2));
        assert_eq!(before, recording.elapsed(), "A pausa acrescentou quadros");
        recording.toggle_pause();
        thread::sleep(Duration::from_secs(1));
        recording.stop();
        let path = recording
            .result
            .recv_timeout(Duration::from_secs(35))
            .unwrap()
            .unwrap();
        let probe = Command::new("ffprobe.exe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height,codec_name,duration",
                "-of",
                "csv=p=0",
            ])
            .arg(&path)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(probe.status.success());
        let output = String::from_utf8(probe.stdout).unwrap();
        let fields: Vec<_> = output.trim().split(',').collect();
        assert_eq!(fields[0], "h264");
        assert!(fields[2].parse::<u32>().unwrap() <= 720);
        let duration: f64 = fields[3].parse().unwrap();
        assert!(
            (1.8..2.7).contains(&duration),
            "Duração incorreta: {duration}"
        );
        println!("Vídeo de teste: {} ({output})", path.display());
        let decode = Command::new(ffmpeg_path())
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "null", "-"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .unwrap();
        assert!(
            decode.status.success(),
            "MP4 não decodificou: {:?}",
            decode.stderr
        );
        fs::remove_file(path).unwrap();
    }
}
