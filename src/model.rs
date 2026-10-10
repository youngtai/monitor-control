use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Monitor {
    pub id: String,
    pub name: String,
    pub serial: String,
    pub connector: String,
    pub current: Option<u16>,
    pub maximum: Option<u16>,
    pub error: Option<String>,
}
impl Monitor {
    pub fn percent(&self) -> Option<u8> {
        self.current
            .zip(self.maximum)
            .filter(|(_, max)| *max > 0)
            .map(|(value, max)| {
                ((u32::from(value) * 100 + u32::from(max) / 2) / u32::from(max)).min(100) as u8
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    Discover,
    Read,
    Set { id: String, percent: u8 },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub monitors: Vec<Monitor>,
    pub error: Option<String>,
}

pub fn raw_value(percent: u8, max: u16) -> u16 {
    ((u32::from(percent.min(100)) * u32::from(max) + 50) / 100) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hardware_ranges_and_rounding() {
        assert_eq!(raw_value(50, 255), 128);
        assert_eq!(raw_value(100, 65535), 65535);
        assert_eq!(raw_value(0, 100), 0);
        assert_eq!(raw_value(255, 100), 100);
    }
}
