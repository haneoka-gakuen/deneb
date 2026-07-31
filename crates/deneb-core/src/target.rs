use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BuildMode {
    Preview,
    Release,
}

impl fmt::Display for BuildMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Preview => "preview",
            Self::Release => "release",
        })
    }
}

impl FromStr for BuildMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "preview" => Ok(Self::Preview),
            "release" => Ok(Self::Release),
            _ => Err(format!("unknown build mode `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum BuildTarget {
    Web,
    Pwa,
    Electron,
    Tauri,
    Android,
    Ios,
}

impl BuildTarget {
    pub const ALL: [Self; 6] = [
        Self::Web,
        Self::Pwa,
        Self::Electron,
        Self::Tauri,
        Self::Android,
        Self::Ios,
    ];

    pub fn is_web_root(self) -> bool {
        matches!(self, Self::Web | Self::Pwa)
    }

    pub fn provider(self) -> &'static str {
        match self {
            Self::Web => "static-web",
            Self::Pwa => "service-worker",
            Self::Electron => "electron",
            Self::Tauri | Self::Android | Self::Ios => "tauri",
        }
    }
}

impl fmt::Display for BuildTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Web => "web",
            Self::Pwa => "pwa",
            Self::Electron => "electron",
            Self::Tauri => "tauri",
            Self::Android => "android",
            Self::Ios => "ios",
        };
        formatter.write_str(value)
    }
}

impl FromStr for BuildTarget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "web" => Ok(Self::Web),
            "pwa" => Ok(Self::Pwa),
            "electron" => Ok(Self::Electron),
            "tauri" => Ok(Self::Tauri),
            "android" => Ok(Self::Android),
            "ios" => Ok(Self::Ios),
            _ => Err(format!("unknown target `{value}`")),
        }
    }
}
