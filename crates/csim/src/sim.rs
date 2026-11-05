//! The applet in jcardsim as a card transport.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::card::{CardError, Transport};
use crate::{hex_upper, parse_hex};

/// The applet sources and Gradle build, `sim/applet` in the checkout this
/// crate was built from. `CSIM_APPLET_DIR` names another.
pub fn applet_dir() -> PathBuf {
    std::env::var_os("CSIM_APPLET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sim/applet"))
}

const READY: &str = "READY";
const SIM_ATR: [u8; 5] = [0x3B, 0x80, 0x80, 0x01, 0x01];

/// `applet/simtest/ApduServer.java`: one hex APDU per line in, the
/// response data and status word as one hex line out. Every connection
/// starts with a freshly installed applet.
pub struct SimTransport {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

fn sim_error(msg: impl Into<String>) -> CardError {
    CardError::Transport(msg.into())
}

impl SimTransport {
    /// Compiles the harness with Gradle, then runs ApduServer under plain
    /// java so that Gradle's console output stays out of the protocol.
    pub fn start() -> Result<SimTransport, CardError> {
        let applet = applet_dir();
        let gradle = Command::new(applet.join("gradlew"))
            .args(["-q", "-p"])
            .arg(&applet)
            .arg("compileSimtest")
            .stdout(Stdio::null())
            .status()
            .map_err(|e| sim_error(format!("running gradlew in {}: {e}", applet.display())))?;
        if !gradle.success() {
            return Err(sim_error("gradlew compileSimtest failed"));
        }
        let classpath_file = applet.join("build/simtest/classpath.txt");
        let classpath = std::fs::read_to_string(&classpath_file)
            .map_err(|e| sim_error(format!("{}: {e}", classpath_file.display())))?;
        let mut child = Command::new("java")
            .args(["-cp", classpath.trim(), "ApduServer"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| sim_error(format!("starting java: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| sim_error("no simulator stdin"))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| sim_error("no simulator stdout"))?,
        );
        let mut sim = SimTransport {
            child,
            stdin,
            stdout,
        };
        let line = sim.read_line()?;
        if line != READY {
            return Err(sim_error(format!("simulator did not start: {line:?}")));
        }
        Ok(sim)
    }

    fn read_line(&mut self) -> Result<String, CardError> {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|e| sim_error(format!("simulator: {e}")))?;
        Ok(line.trim().to_owned())
    }
}

impl Transport for SimTransport {
    fn transmit(&mut self, apdu: &[u8]) -> Result<Vec<u8>, CardError> {
        writeln!(self.stdin, "{}", hex_upper(apdu))
            .map_err(|e| sim_error(format!("simulator: {e}")))?;
        self.stdin
            .flush()
            .map_err(|e| sim_error(format!("simulator: {e}")))?;
        let line = self.read_line()?;
        if line.starts_with("ERR") || line.len() < 4 {
            return Err(sim_error(format!("simulator: {line}")));
        }
        parse_hex(&line).map_err(|e| sim_error(format!("simulator: {e}")))
    }

    fn atr(&self) -> Vec<u8> {
        SIM_ATR.to_vec()
    }

    fn name(&self) -> String {
        "jcardsim".into()
    }
}

impl Drop for SimTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
