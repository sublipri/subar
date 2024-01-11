use std::env::{self, args};
use std::future::Future;

use anyhow::Result;
use chrono::Local;
use mpd_client::{commands, Client};
use serde::Serialize;
use tokio::net::{TcpStream, UnixStream};
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration};
use unicode_segmentation::UnicodeSegmentation;

static MPD_DEFAULT_HOST: &str = "/run/mpd/socket";
static MPD_FALLBACK: &str = "🎵 ???";
static VOL_FALLBACK: &str = "🔊 ???";
static WEATHER_FALLBACK: &str = "🛰️ ???";
static MAIN_UDPDATE_FREQUENCY: u64 = 100;
static MPD_UPDATE_FREQUENCY: u64 = 112;
static VOL_UPDATE_FREQUENCY: u64 = 323;
static WEATHER_UPDATE_FREQUENCY: u64 = 5137;
static NOW_PLAYING_MAX_LEN: usize = 70;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut tasks = Vec::new();
    if !args().any(|a| a == "--no-mpd") {
        tasks.push(Taskmaster::new(mpd_task, MPD_FALLBACK));
    }
    if !args().any(|a| a == "--no-vol") {
        tasks.push(Taskmaster::new(volume_task, VOL_FALLBACK));
    }
    if !args().any(|a| a == "--no-bom") {
        tasks.push(Taskmaster::new(weather_task, WEATHER_FALLBACK));
    }

    sleep(Duration::from_millis(20)).await;
    let mut header = Header::default();
    if args().any(|a| a == "--no-stop-on-hide") {
        header.cont_signal = 0;
        header.stop_signal = 0;
    }
    println!("{}", serde_json::to_string(&header).unwrap());
    println!("[");
    let mut status = StatusLine::default();
    loop {
        for task in &tasks {
            status.full_text.push_str(&task.status());
            status.full_text.push(' ');
        }
        let now = Local::now();
        let datetime = now.format("🗓️ %a %b %d 🕛 %T").to_string();
        status.full_text.push_str(&datetime);

        println!("[{}],", serde_json::to_string(&status).unwrap());
        sleep(Duration::from_millis(MAIN_UDPDATE_FREQUENCY)).await;
        status.full_text.clear();
    }
}

#[derive(Serialize)]
struct Header {
    version: u8,
    click_events: bool,
    cont_signal: u8,
    stop_signal: u8,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            version: 1,
            click_events: false,
            cont_signal: 18,
            stop_signal: 19,
        }
    }
}

#[derive(Default, Serialize)]
struct StatusLine {
    full_text: String,
}

pub struct Taskmaster {
    _handle: JoinHandle<Result<()>>,
    rx: watch::Receiver<String>,
}

type TaskFn<R> = fn(watch::Sender<String>) -> R;

impl Taskmaster {
    pub fn new<'a>(
        task_fn: TaskFn<impl Future<Output = Result<()>> + Send + 'a + 'static>,
        fallback: &'a str,
    ) -> Self {
        let (tx, rx) = watch::channel(fallback.to_string());
        let _handle = tokio::spawn(task_fn(tx));
        Self { _handle, rx }
    }
    pub fn status(&self) -> watch::Ref<'_, String> {
        self.rx.borrow()
    }
}

async fn weather_task(tx: watch::Sender<String>) -> Result<()> {
    let mut bom_args = vec!["current"];
    if args().any(|a| a == "--check-weather") {
        bom_args.push("--check");
    }
    loop {
        let Ok(cmd) = Command::new("bom-buddy").args(&bom_args).output().await else {
            tx.send(WEATHER_FALLBACK.to_string())?;
            sleep(Duration::from_millis(WEATHER_UPDATE_FREQUENCY)).await;
            continue;
        };
        let weather = if cmd.status.success() {
            String::from_utf8(cmd.stdout)?
        } else {
            WEATHER_FALLBACK.to_string()
        };
        tx.send(weather)?;
        sleep(Duration::from_millis(WEATHER_UPDATE_FREQUENCY)).await;
    }
}

async fn volume_task(tx: watch::Sender<String>) -> Result<()> {
    loop {
        let Ok(cmd) = Command::new("wpctl")
            .arg("get-volume")
            .arg("@DEFAULT_AUDIO_SINK@")
            .output()
            .await
        else {
            tx.send(VOL_FALLBACK.to_string())?;
            sleep(Duration::from_millis(1000)).await;
            continue;
        };

        if !cmd.status.success() {
            tx.send(VOL_FALLBACK.to_string())?;
            sleep(Duration::from_millis(1000)).await;
            continue;
        }

        let output = String::from_utf8(cmd.stdout)?;

        let icon = if output.contains("MUTED") {
            "🔇"
        } else {
            "🔊"
        };

        let volume = &output.trim()[10..12];
        let status = format!("{} {}%", icon, volume);
        tx.send(status)?;
        sleep(Duration::from_millis(VOL_UPDATE_FREQUENCY)).await;
    }
}

async fn mpd_task(tx: watch::Sender<String>) -> Result<()> {
    let host = if let Ok(host) = env::var("MPD_HOST") {
        host
    } else {
        MPD_DEFAULT_HOST.to_string()
    };
    loop {
        let connection = if host.starts_with('/') {
            match UnixStream::connect(&host).await {
                Ok(conn) => Client::connect(conn).await,
                Err(err) => {
                    eprintln!("Couldn't connect to {host}. {err}");
                    sleep(Duration::from_millis(1000)).await;
                    continue;
                }
            }
        } else {
            match TcpStream::connect(&host).await {
                Ok(conn) => Client::connect(conn).await,
                Err(err) => {
                    eprintln!("Couldn't connect to {host}. {err}");
                    sleep(Duration::from_millis(1000)).await;
                    continue;
                }
            }
        };

        let (client, _) = match connection {
            Ok(ok) => ok,
            Err(err) => {
                eprintln!("Couldn't connect to {host}. {err}");
                sleep(Duration::from_millis(1000)).await;
                continue;
            }
        };

        loop {
            let Ok(now_playing) = get_now_playing(&client).await else {
                tx.send(MPD_FALLBACK.to_string())?;
                break;
            };
            tx.send(now_playing)?;
            sleep(Duration::from_millis(MPD_UPDATE_FREQUENCY)).await;
        }
    }
}

async fn get_now_playing(client: &Client) -> Result<String> {
    let Some(current) = client.command(commands::CurrentSong).await? else {
        return Ok(MPD_FALLBACK.to_string());
    };

    let status = client.command(commands::Status).await?;
    let artists = current.song.artists();
    let album_artist = current.song.album_artists();
    let artists = if artists.is_empty() && !album_artist.is_empty() {
        album_artist
    } else {
        artists
    };
    let title = if let Some(title) = current.song.title() {
        title
    } else {
        "???"
    };
    let artist = match artists.len() {
        0 => "???".to_string(),
        1 => artists[0].to_string(),
        2 => artists.join(" & "),
        _ => artists.join(", "),
    };
    let mut playing = format!("{artist} - {title}");
    if playing.len() > NOW_PLAYING_MAX_LEN {
        let mut iter = playing.grapheme_indices(true);
        if let Some((offset, _)) = iter.nth(NOW_PLAYING_MAX_LEN) {
            let idx = playing[..offset].trim_end().len();
            playing.truncate(idx);
            playing.push('…');
        }
    };

    let playback_time = if let Some(elapsed) = status.elapsed {
        let elapsed = format_duration(elapsed);
        let duration = format_duration(status.duration.unwrap());
        format!("{elapsed}/{duration}")
    } else {
        "00:00".to_string()
    };

    Ok(format!("🎵 {playing} ({playback_time})"))
}

fn format_duration(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}
