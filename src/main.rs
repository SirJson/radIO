use logind_zbus::ManagerProxy;
use zbus::Connection;

use std::{
    collections::HashMap,
    fs::{self, File},
    io::Read,
    thread,
    time::Duration,
};

use futures::stream::StreamExt;
use gpio_cdev::{AsyncLineEventHandle, Chip, EventRequestFlags, EventType, LineRequestFlags};
use log::{debug, error, info, warn, LevelFilter};
use serde::{Deserialize, Serialize};

use simplelog::{ColorChoice, CombinedLogger, Config, TermLogger, TerminalMode, WriteLogger};

const DEFAULT_CHIP: &str = "/dev/gpiochip0";
const CFGPATH: &str = "/etc/radio.conf";

type Void = anyhow::Result<()>;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct AppConfig {
    pub master_chip: String,
    pub log_level: u8,
    pub input_binding: HashMap<String, String>,
    pub output_binding: HashMap<String, String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            master_chip: DEFAULT_CHIP.to_string(),
            log_level: 3,
            input_binding: HashMap::new(),
            output_binding: HashMap::new(),
        }
    }
}

fn poweroff() -> Void {
    let connection = Connection::new_system()?;
    let manager = ManagerProxy::new(&connection)?;
    manager.power_off(false)?;
    Ok(())
}

fn halt() -> Void {
    let connection = Connection::new_system()?;
    let manager = ManagerProxy::new(&connection)?;
    manager.halt(false)?;
    Ok(())
}

fn reboot() -> Void {
    let connection = Connection::new_system()?;
    let manager = ManagerProxy::new(&connection)?;
    manager.reboot(false)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Void {
    let cfg = sanitise_gpio_names(load_config()?);
    init_log(&cfg)?;

    info!("{} - {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    info!("Using {}", cfg.master_chip);
    let mut chip = Chip::new(cfg.master_chip)?;
    info!("Setup outputs");
    for (gpio, func) in cfg.output_binding {
        exec_binding(&func, &mut chip, gpio.parse::<u32>()?).await?
    }
    info!("Setup input");
    let mut input_events: Vec<(AsyncLineEventHandle, String, u32)> = Vec::new();
    for (gpio, func) in cfg.input_binding {
        let event = get_evt_handle(&mut chip, gpio.parse::<u32>()?)?;
        input_events.push((event, func, gpio.parse::<u32>()?));
    }
    info!("Start event handler");
    tick(&mut input_events, &mut chip).await?;
    Ok(())
}

fn sanitise_gpio_names(cfg: AppConfig) -> AppConfig {
    let mut appcfg = cfg.clone();
    let out_cloned = appcfg.output_binding.clone();
    appcfg.output_binding.clear();
    for (k, v) in out_cloned {
        appcfg
            .output_binding
            .insert(k.replace("gpio", "").trim().to_string(), v.clone());
    }

    let in_cloned = appcfg.input_binding.clone();
    appcfg.input_binding.clear();
    for (k, v) in in_cloned {
        appcfg
            .input_binding
            .insert(k.replace("gpio", "").trim().to_string(), v.clone());
    }
    appcfg
}

fn load_config() -> anyhow::Result<AppConfig> {
    match File::open(CFGPATH) {
        Ok(mut f) => {
            let mut buffer = String::default();
            f.read_to_string(&mut buffer)?;
            let appcfg: AppConfig = toml::from_str(&buffer)?;

            Ok(appcfg)
        }
        Err(e) => {
            warn!("{}", e);
            let defaultcfg = AppConfig::default();
            info!("Writing default config to {}", CFGPATH);
            let toml = toml::to_string_pretty(&defaultcfg)?;
            fs::write(CFGPATH, toml)?;
            Ok(defaultcfg)
        }
    }
}

fn log_level_to_enum(input: u8) -> LevelFilter {
    match input {
        0 => LevelFilter::Off,
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        5 => LevelFilter::Trace,
        _ => LevelFilter::Error,
    }
}

fn get_evt_handle(chip: &mut Chip, gpio: u32) -> anyhow::Result<AsyncLineEventHandle> {
    let handle = chip.get_line(gpio)?;
    let evt = AsyncLineEventHandle::new(handle.events(
        LineRequestFlags::INPUT,
        EventRequestFlags::BOTH_EDGES,
        &format!("gpio_event_{}", gpio),
    )?)?;
    Ok(evt)
}

async fn exec_binding(input: &str, chip: &mut Chip, gpio: u32) -> Void {
    match input {
        "poweroff" | "shutdown" => {
            debug!("Sync file system...");
            nix::unistd::sync();
            thread::sleep(Duration::from_secs(2));
            warn!("The system will poweroff NOW");
            poweroff()?;
        }
        "restart" => {
            debug!("Sync file system...");
            nix::unistd::sync();
            thread::sleep(Duration::from_secs(2));

            reboot()?;
        }
        "halt" => {
            debug!("Sync file system...");
            nix::unistd::sync();
            thread::sleep(Duration::from_secs(2));
            halt()?;
        }
        "seton" => static_line(chip, gpio, true).await?,
        "setoff" => static_line(chip, gpio, false).await?,
        _ => error!("Unknown function"),
    }
    Ok(())
}

async fn static_line(chip: &mut Chip, gpionum: u32, state: bool) -> Void {
    let line = chip.get_line(gpionum)?;
    info!("Setup GPIO {} output with default", &state);
    line.request(
        LineRequestFlags::OUTPUT,
        u8::from(state),
        &format!("static_gpio_{}", gpionum),
    )?;
    Ok(())
}

fn init_log(cfg: &AppConfig) -> anyhow::Result<()> {
    let loglevel = log_level_to_enum(cfg.log_level);
    CombinedLogger::init(vec![
        TermLogger::new(
            loglevel,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            loglevel,
            Config::default(),
            File::create(format!("/var/log/{}.log", env!("CARGO_PKG_NAME"))).unwrap(),
        ),
    ])?;
    Ok(())
}

async fn tick(
    events: &mut Vec<(AsyncLineEventHandle, String, u32)>,
    chip: &mut Chip,
) -> anyhow::Result<()> {
    debug!("Event loop started");

    loop {
        for evt in events.into_iter() {
            debug!("Checking GPIO {} for events", &evt.2);
            match evt.0.next().await {
                Some(event) => {
                    let info = event?;
                    if info.event_type() == EventType::FallingEdge {
                        debug!("Execute {}", &evt.2);
                        exec_binding(&evt.1, chip, evt.2).await?;
                    }
                }
                None => continue,
            }
        }
    }
}
