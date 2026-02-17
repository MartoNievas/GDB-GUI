use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{Receiver, Sender},
    thread,
};

use super::parser::parse_line;
use super::writer::command_to_mi;
use crate::state::{DebuggerEvent, UiEvent};
use crate::ui::command::Command as DebuggerCommand;

pub struct GdbProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    seq: u32,
}

impl GdbProcess {
    // the commnad gdb launch --interpreter=mi <excutable> and return the proccess ready

    pub fn spawn(executable: Option<&str>) -> std::io::Result<Self> {
        let mut cmd = Command::new("gdb");
        cmd.arg("--interpreter=mi")
            .arg("--quiet")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        if let Some(exe) = executable {
            cmd.arg(exe);
        }

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout_raw = child.stdout.take().expect("stdout piped");
        let stdout = BufReader::new(stdout_raw);

        Ok(Self {
            child,
            stdin,
            stdout,
            seq: 1,
        })
    }

    pub fn send(&mut self, raw_in: str&) -> std::io::Result<u32> {
        let token = self.seq;
        self.seq +=1;

        writeln!(self.stdin,"{}{}",token,raw_in)?;
        self.stdin.flush()?;
        Ok(token)
    }
}
