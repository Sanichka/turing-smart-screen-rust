// SPDX-License-Identifier: GPL-3.0-or-later
//! Custom data sources. Ports `sensors_custom.py`: theme `CUSTOM` sections
//! name a class; the daemon instantiates it by name and renders its
//! TEXT/GRAPH/RADIAL/LINE_GRAPH widgets.
//!
//! Rust has no runtime Python import, so sources are compiled-in behind a
//! small registry (`CustomDataSource` + `CustomBank`). Adding your own:
//! implement the trait, register the name in `CustomBank::new()`.

use std::collections::HashMap;

/// One poll of a custom source (mirrors the three Python methods).
#[derive(Debug, Clone, Default)]
pub struct CustomReading {
    pub name: String,
    pub numeric: Option<f32>,
    pub string: Option<String>,
    pub history: Vec<f32>,
}

pub trait CustomDataSource: Send {
    fn as_numeric(&mut self) -> Option<f32>;
    fn as_string(&mut self) -> Option<String>;
    /// History oldest→newest for line graphs (NaN-padded like Python).
    fn history(&self) -> Vec<f32>;
}

/// Push `v` into a NaN-initialized ring of `size`, return a snapshot.
fn push_history(buf: &mut Vec<f32>, v: f32, size: usize) -> Vec<f32> {
    if buf.len() != size {
        *buf = vec![f32::NAN; size];
    }
    buf.push(v);
    buf.remove(0);
    buf.clone()
}

/// Port of `ExampleCustomNumericData` (fixed demo value + history).
pub struct ExampleNumeric {
    value: f32,
    last: Vec<f32>,
}

impl ExampleNumeric {
    fn new() -> Self {
        ExampleNumeric { value: 0.0, last: vec![f32::NAN; 10] }
    }
}

impl CustomDataSource for ExampleNumeric {
    fn as_numeric(&mut self) -> Option<f32> {
        self.value = 75.845;
        self.last = push_history(&mut self.last, self.value, 10);
        Some(self.value)
    }

    fn as_string(&mut self) -> Option<String> {
        Some(format!("{:>5.1}%", self.value))
    }

    fn history(&self) -> Vec<f32> {
        self.last.clone()
    }
}

/// Port of `ExampleCustomTextOnlyData` (text only: platform description).
pub struct ExampleText;

impl CustomDataSource for ExampleText {
    fn as_numeric(&mut self) -> Option<f32> {
        None
    }

    fn as_string(&mut self) -> Option<String> {
        Some(format!("{} {}", std::env::consts::OS, std::env::consts::ARCH))
    }

    fn history(&self) -> Vec<f32> {
        Vec::new()
    }
}

/// Named registry polled once per tick; readings land on the snapshot so
/// rendering stays a pure function of `(theme, snapshot)`.
pub struct CustomBank {
    sources: HashMap<String, Box<dyn CustomDataSource>>,
}

impl CustomBank {
    pub fn new() -> Self {
        let mut sources: HashMap<String, Box<dyn CustomDataSource>> = HashMap::new();
        sources.insert("ExampleCustomNumericData".into(), Box::new(ExampleNumeric::new()));
        sources.insert("ExampleCustomTextOnlyData".into(), Box::new(ExampleText));
        CustomBank { sources }
    }

    pub fn read_all(&mut self) -> Vec<CustomReading> {
        let mut out = Vec::with_capacity(self.sources.len());
        let mut names: Vec<String> = self.sources.keys().cloned().collect();
        names.sort();
        for name in names {
            if let Some(s) = self.sources.get_mut(&name) {
                let numeric = s.as_numeric();
                let string = s.as_string().or_else(|| numeric.map(|v| v.to_string()));
                out.push(CustomReading { name, numeric, string, history: s.history() });
            }
        }
        out
    }
}

impl Default for CustomBank {
    fn default() -> Self {
        Self::new()
    }
}
