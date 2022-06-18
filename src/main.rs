pub(crate) use anyhow::{bail, Context, Error, Result};
use getopt::Opt;
use notify_rust::{Notification, Timeout, Urgency};
use std::fs;
use std::path::Path;
use std::process::{exit, Command};
use std::str::FromStr;
use std::{thread, time};

const VERSION: &str = "0.1";
const PROGNAME: &str = "batsignal";
const POWER_SUPLY_DIR: &str = "/sys/class/power_supply";

#[derive(Debug, PartialEq)]
enum State {
    Charging,
    Discharging,
    Warning,
    Critical,
    Danger,
    Full,
}

#[derive(Debug, PartialEq)]
enum BatteryStatus {
    Unknown,
    Charging,
    Discharging,
    NotCharging,
    Full,
}

impl FromStr for BatteryStatus {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "Unknown" => Ok(BatteryStatus::Unknown),
            "Charging" => Ok(BatteryStatus::Charging),
            "Discharging" => Ok(BatteryStatus::Discharging),
            "Not charging" => Ok(BatteryStatus::NotCharging),
            "Full" => Ok(BatteryStatus::Full),
            other => Err(Error::msg(format!(
                "Failed to parse battery status, found {other}"
            ))),
        }
    }
}

#[derive(Debug)]
struct Battery {
    name: String,
    status: BatteryStatus,
    energy_full: i32,
    energy_now: i32,
}

impl Battery {
    fn new<S: Into<String>>(name: S) -> Result<Self> {
        let name = name.into();
        if Path::new(POWER_SUPLY_DIR).join(name.as_str()).exists() {
            Ok(Self {
                name,
                status: BatteryStatus::Discharging,
                energy_full: 0,
                energy_now: 0,
            })
        } else {
            bail!("Battery {name} not found")
        }
    }
}

#[derive(Debug)]
struct Settings {
    daemonize: bool,
    run_once: bool,

    batteries: Vec<Battery>,

    sleep_interval: i32,

    warning: Option<i32>,
    critical: Option<i32>,
    danger: Option<i32>,
    full: Option<i32>,

    warningmsg: String,
    criticalmsg: String,
    fullmsg: String,

    dangercmd: Option<String>,
    appname: String,
    icon: Option<String>,
    notification_timeout: Timeout,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            daemonize: true,
            run_once: false,

            batteries: Vec::new(),

            sleep_interval: 60,

            warning: Some(15),
            critical: Some(5),
            danger: Some(2),
            full: None,

            warningmsg: "Battery is low".to_string(),
            criticalmsg: "Battery is critically low".to_string(),
            fullmsg: "Battery is full".to_string(),

            dangercmd: None,
            appname: PROGNAME.to_string(),
            icon: None,
            notification_timeout: Timeout::Never,
        }
    }
}

impl Settings {
    fn validate(self) -> Result<Self> {
        macro_rules! rangeerror {
            ($var:expr, $hi:expr) => {
                Err(Error::msg(format!(
                    "Option -{} must be between 0 and {}",
                    $var, $hi
                )))
            };
        }

        if let Some(warning) = self.warning {
            if !(0..=100).contains(&warning) {
                return rangeerror!("w", 100);
            }
        }
        if let Some(critical) = self.critical {
            if !(0..=100).contains(&critical) {
                return rangeerror!("c", 100);
            }
        }
        if let Some(full) = self.full {
            if !(0..=100).contains(&full) {
                return rangeerror!("f", 100);
            }
        }

        if self.sleep_interval < 0 {
            return rangeerror!("m", i32::MAX / 1000);
        }

        if let (Some(warning), Some(critical)) = (self.warning, self.critical) {
            if warning <= critical {
                return Err(Error::msg("Warning level must be greater than critical"));
            }
        }

        let mut vals = [
            ("danger", self.danger),
            ("critical", self.critical),
            ("warning", self.warning),
            ("full", self.full),
        ]
        .into_iter()
        .filter_map(|(name, opt)| opt.map(|value| (name, value)));

        // We can safely unwrap here because the iterator is not empty
        let mut greatest = vals.next().unwrap();
        for v in vals {
            if v.1 >= greatest.1 {
                greatest = v
            } else {
                return Err(Error::msg(format!(
                    "{} must be greater than {}",
                    v.0, greatest.0
                )));
            }
        }

        Ok(self)
    }
}

fn print_help() {
    print!(
        "Usage: {PROGNAME} [OPTIONS]\n\
    \n\
    Sends battery level notifications.\n\
    \n\
    Options:
    -h             print this help message
    -v             print program version information
    -b             run as background daemon
    -o             check battery once and exit
    -e             cause notifications to expire
    -w LEVEL       battery warning LEVEL
                   (default: 15)
    -c LEVEL       critical battery LEVEL
                   (default: 5)
    -d LEVEL       battery danger LEVEL
                   (default: 2)
    -f LEVEL       full battery LEVEL
                   (default: disabled)
    -W MESSAGE     show MESSAGE when battery is at warning level
    -C MESSAGE     show MESSAGE when battery is at critical level
    -D COMMAND     run COMMAND when battery is at danger level
    -F MESSAGE     show MESSAGE when battery is full
    -n NAME        use battery NAME - multiple batteries separated by commas
                   (default: BAT0)
    -s SECONDS     number of SECONDS to wait between battery checks
                   (default: 60)
    -a NAME        app NAME used in desktop notifications
                   (default: {PROGNAME})
    -I ICON        display specified ICON in notifications\n\
    "
    )
}

fn print_version() {
    println!("{PROGNAME} {VERSION}")
}

fn handle_battery_names(settings: &mut Settings, battery_names: &str) -> Result<()> {
    settings.batteries = battery_names
        .replace(' ', "")
        .split(',')
        .map(Battery::new)
        .collect::<Result<Vec<Battery>>>()?;

    Ok(())
}

fn parse_args() -> Result<Settings> {
    let mut settings = Settings::default();

    let args: Vec<String> = std::env::args().collect();
    let mut opts = getopt::Parser::new(&args, "hvboew:c:d:f:W:C:D:F:n:s:a:I:");

    loop {
        match opts
            .next()
            .transpose()
            .with_context(|| "Failed to parse args")?
        {
            None => break,
            Some(opt) => match opt {
                Opt('h', None) => {
                    print_help();
                    exit(0);
                }
                Opt('v', None) => {
                    print_version();
                    exit(0);
                }
                Opt('b', None) => settings.daemonize = true,
                Opt('o', None) => settings.run_once = true,
                Opt('w', Some(warning)) => {
                    settings.warning = Some(
                        warning
                            .parse()
                            .with_context(|| "Error parsing argument for option w")?,
                    )
                }
                Opt('c', Some(critical)) => {
                    settings.critical = Some(
                        critical
                            .parse()
                            .with_context(|| "Error parsing argument for option c")?,
                    )
                }
                Opt('d', Some(danger)) => {
                    settings.danger = Some(
                        danger
                            .parse()
                            .with_context(|| "Error parsing argument for option d")?,
                    )
                }
                Opt('f', Some(full)) => {
                    settings.full = Some(
                        full.parse()
                            .with_context(|| "Error parsing argument for option f")?,
                    )
                }
                Opt('W', Some(warningmsg)) => settings.warningmsg = warningmsg,
                Opt('C', Some(criticalmsg)) => settings.criticalmsg = criticalmsg,
                Opt('D', dangercmd) => settings.dangercmd = dangercmd,
                Opt('F', Some(fullmsg)) => settings.fullmsg = fullmsg,
                Opt('n', Some(battery_names)) => {
                    handle_battery_names(&mut settings, battery_names.as_str())?
                }
                Opt('s', Some(sleep_interval)) => {
                    settings.sleep_interval = sleep_interval
                        .parse()
                        .with_context(|| "Error parsing argument for option m")?
                }
                Opt('a', Some(appname)) => settings.appname = appname,
                Opt('I', icon) => settings.icon = icon,
                Opt('e', None) => settings.notification_timeout = Timeout::Default,
                _ => unreachable!(),
            },
        }
    }

    Ok(settings)
}

fn find_batteries() -> Result<Vec<Battery>> {
    let mut found_batteries: Vec<Battery> = Vec::new();

    for f in fs::read_dir(POWER_SUPLY_DIR)? {
        let f_path = f?.path();

        if f_path.is_dir()
            && f_path.join("type").exists()
            && fs::read_to_string(f_path.join("type"))?.contains("Battery")
        {
            found_batteries.push(Battery::new(
                f_path
                    .file_name()
                    .ok_or_else(|| anyhow::Error::msg("Invalid file name"))?
                    .to_str()
                    .ok_or_else(|| {
                        anyhow::Error::msg("Failed to convert battery name to string")
                    })?,
            )?);
        }
    }

    if !found_batteries.is_empty() {
        Ok(found_batteries)
    } else {
        Err(Error::msg("No batteries found"))
    }
}

fn update_batteries(batteries: &mut Vec<Battery>) -> Result<()> {
    for battery in batteries {
        let path = Path::new(POWER_SUPLY_DIR).join(battery.name.as_str());
        battery.energy_now = fs::read_to_string(path.join("energy_now"))?
            .trim()
            .parse()
            .with_context(|| format!("Error parsing energy_now for {}", battery.name))?;

        battery.energy_full = fs::read_to_string(path.join("energy_full"))?
            .trim()
            .parse()
            .with_context(|| format!("Error parsing energy_full for {}", battery.name))?;

        battery.status = fs::read_to_string(path.join("status"))?
            .trim()
            .parse()
            .with_context(|| format!("Error parsing status for {}", battery.name))?;
    }

    Ok(())
}

fn notify_cmd(settings: &Settings, state: &State, charge_percent: i32) -> Result<()> {
    let mut notification = Notification::new()
        .timeout(settings.notification_timeout)
        .appname(settings.appname.as_str())
        .finalize();

    if settings.icon.is_some() {
        notification = notification
            .icon(settings.icon.clone().unwrap().as_str())
            .finalize();
    }

    let summary: &str;
    let mut urgency = Urgency::Normal;
    let body = format!("Battery level: {}%", charge_percent);
    match state {
        State::Warning => summary = settings.warningmsg.as_str(),
        State::Critical => {
            summary = settings.criticalmsg.as_str();
            urgency = Urgency::Critical;
        }
        State::Full => summary = settings.fullmsg.as_str(),
        State::Danger => {
            if settings.dangercmd.is_some() {
                Command::new("sh")
                    .arg("-c")
                    .arg(settings.dangercmd.as_ref().unwrap())
                    .spawn()
                    .with_context(|| {
                        format!("Failed to run {}", settings.dangercmd.clone().unwrap())
                    })?;
            }

            return Ok(());
        }
        _ => return Ok(()),
    }

    notification
        .body(body.as_str())
        .summary(summary)
        .urgency(urgency)
        .show()
        .with_context(|| "Failed to show notification")?;
    Ok(())
}

fn main() -> Result<()> {
    let mut settings = parse_args()?.validate()?;
    if settings.batteries.is_empty() {
        settings.batteries = find_batteries()?
    }

    let batteries = settings
        .batteries
        .iter()
        .map(|b| b.name.clone())
        .reduce(|accum: String, item: String| format!("{}, {}", accum, item))
        .unwrap(); // We can unwrap here because finding no batteries is already handled before

    println!("Using batteries {batteries}");

    let mut charge: (f64, f64);
    let mut charge_percent: i32;
    let mut discharging: bool;
    let mut state = State::Discharging;
    let mut new_state: State;

    loop {
        update_batteries(&mut settings.batteries)?;

        charge = settings
            .batteries
            .iter()
            .map(|b| b.energy_now as f64)
            .zip(settings.batteries.iter().map(|b| b.energy_full as f64))
            .reduce(|accum, item| (accum.0 + item.0, accum.1 + item.1))
            .unwrap();
        charge_percent = (charge.0 / charge.1 * 100.0) as i32;

        discharging = settings
            .batteries
            .iter()
            .any(|b| b.status == BatteryStatus::Discharging);

        if !discharging {
            if settings.full.is_some() && charge_percent >= settings.full.unwrap() {
                new_state = State::Full;
            } else {
                new_state = State::Charging;
            }
        } else {
            new_state = [
                (State::Discharging, Some(100)),
                (State::Warning, settings.warning),
                (State::Critical, settings.critical),
                (State::Danger, settings.danger),
            ]
            .into_iter()
            .filter_map(|(state, opt)| opt.map(|level| (state, level)))
            .reduce(|accum, item| {
                if charge_percent <= item.1 && item.1 <= accum.1 {
                    item
                } else {
                    accum
                }
            })
            .unwrap()
            .0;
        }

        if new_state != state {
            notify_cmd(&settings, &new_state, charge_percent)?;
            state = new_state;
        }

        if settings.run_once {
            break;
        } else {
            thread::sleep(time::Duration::from_secs(settings.sleep_interval as u64));
        }
    }

    Ok(())
}
