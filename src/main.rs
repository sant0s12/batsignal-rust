use anyhow::{bail, Context, Error, Result};
use getopt::Opt;
use notify_rust::{Notification, Timeout};
use std::fs;
use std::path::Path;
use std::process::exit;

const VERSION: &str = "0.1";
const PROGNAME: &str = "batsignal";
const POWER_SUPLY_DIR: &str = "/sys/class/power_supply";

#[derive(Debug)]
enum BatteryState {
    AC,
    Discharging,
    Warning,
    Critical,
    Danger,
    Full,
}

#[derive(Debug)]
struct Battery {
    name: String,
    state: BatteryState,
    level: i32,
    energy_full: i32,
    energy_now: i32,
}

impl Battery {
    fn new<S: Into<String>>(name: S) -> Result<Self> {
        let name = name.into();
        if Path::new(format!("{POWER_SUPLY_DIR}/{name}").as_str()).exists() {
            Ok(Self {
                name,
                state: BatteryState::Discharging,
                level: 0,
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
    battery_required: bool,

    batteries: Vec<Battery>,

    multiplier: i32,

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
            battery_required: true,

            batteries: Vec::new(),

            multiplier: 0,

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
            if warning > 100 || warning < 0 {
                return rangeerror!("w", 100);
            }
        }
        if let Some(critical) = self.critical {
            if critical > 100 || critical < 0 {
                return rangeerror!("c", 100);
            }
        }
        if let Some(full) = self.full {
            if full > 100 || full < 0 {
                return rangeerror!("f", 100);
            }
        }

        if self.multiplier > 3600 || self.multiplier < 0 {
            return rangeerror!("m", 3600);
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
        .filter_map(|(name, opt)| {
            if let Some(value) = opt {
                Some((name, value))
            } else {
                None
            }
        });

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
    -i             ignore missing battery errors
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
    -m SECONDS     minimum number of SECONDS to wait between battery checks
                   0 SECONDS disables polling and waits for USR1 signal
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
        .replace(" ", "")
        .split(",")
        .map(|battery_name| Ok(Battery::new(battery_name)?))
        .collect::<Result<Vec<Battery>>>()?;

    Ok(())
}

fn parse_args() -> Result<Settings> {
    let mut settings = Settings::default();

    let args: Vec<String> = std::env::args().collect();
    let mut opts = getopt::Parser::new(&args, "hvboiew:c:d:f:W:C:D:F:n:m:a:I:");

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
                Opt('i', None) => settings.battery_required = false,
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
                Opt('m', Some(multiplier)) => {
                    settings.multiplier = multiplier
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
                    .ok_or(anyhow::Error::msg("Invalid file name"))?
                    .to_str()
                    .ok_or(anyhow::Error::msg(
                        "Failed to convert battery name to string",
                    ))?,
            )?);
        }
    }

    if !found_batteries.is_empty() {
        Ok(found_batteries)
    } else {
        Err(Error::msg("No batteries found"))
    }
}

fn update_batteries(batteries: &mut Vec<Battery>) {}

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

    loop {
        update_batteries(&mut settings.batteries);

        if settings.run_once {
            break;
        }
    }

    Ok(())
}
