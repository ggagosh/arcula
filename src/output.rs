use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::Error;
use clap::ValueEnum;
use serde::Serialize;

static OUTPUT_FORMAT: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
            Self::Json => write!(f, "json"),
        }
    }
}

impl OutputFormat {
    pub fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }

    pub fn is_text(self) -> bool {
        matches!(self, Self::Text)
    }
}

pub fn set_output_format(format: OutputFormat) {
    OUTPUT_FORMAT.store(
        match format {
            OutputFormat::Text => 0,
            OutputFormat::Json => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn current_output_format() -> OutputFormat {
    match OUTPUT_FORMAT.load(Ordering::Relaxed) {
        1 => OutputFormat::Json,
        _ => OutputFormat::Text,
    }
}

pub fn is_text() -> bool {
    current_output_format().is_text()
}

pub fn is_json() -> bool {
    current_output_format().is_json()
}

#[derive(Serialize)]
struct JsonSuccess<'a, T>
where
    T: Serialize + ?Sized,
{
    ok: bool,
    kind: &'a str,
    data: &'a T,
}

#[derive(Serialize)]
struct JsonErrorResponse {
    ok: bool,
    error: JsonErrorBody,
}

#[derive(Serialize)]
struct JsonErrorBody {
    code: &'static str,
    message: String,
}

pub fn print_json_success<T>(kind: &'static str, data: &T)
where
    T: Serialize + ?Sized,
{
    let response = JsonSuccess {
        ok: true,
        kind,
        data,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&response).expect("JSON serialization failed")
    );
}

pub fn print_json_error(code: &'static str, error: &Error) {
    let response = JsonErrorResponse {
        ok: false,
        error: JsonErrorBody {
            code,
            message: format!("{error:#}"),
        },
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&response).expect("JSON serialization failed")
    );
}
