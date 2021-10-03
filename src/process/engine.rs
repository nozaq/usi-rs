use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::BufReader;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;

use super::reader::{EngineCommandReader, EngineOutput};
use super::writer::GuiCommandWriter;
use crate::error::Error;
use crate::protocol::*;

/// Represents a metadata returned from a USI engine.
#[derive(Clone, Debug, Default)]
pub struct EngineInfo {
    name: String,
    options: HashMap<String, String>,
}

impl EngineInfo {
    /// Returns an engine name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns available engine options.
    pub fn options(&self) -> &HashMap<String, String> {
        &self.options
    }
}

/// `UsiEngineHandler` provides a type-safe interface to the USI engine process.
///
/// # Examples
/// ```no_run
/// use usi::{BestMoveParams, Error, EngineCommand, GuiCommand, UsiEngineHandler};
///
/// let mut handler = UsiEngineHandler::spawn("/path/to/usi_engine", "/path/to/working_dir").unwrap();
///
/// // Get the USI engine information.
/// let info = handler.prepare().unwrap();
/// assert_eq!("engine name", info.name());
///
/// // Set options.
/// handler.send_command(&GuiCommand::SetOption("USI_Ponder".to_string(), Some("true".to_string()))).unwrap();
///
/// // Start listening to the engine output.
/// // You can pass the closure which will be called
/// //   everytime new command is received from the engine.
/// handler.listen(move |output| -> Result<(), Error> {
///     match output.response() {
///         Some(EngineCommand::BestMove(BestMoveParams::MakeMove(
///                      ref best_move_sfen,
///                      ref ponder_move,
///                 ))) => {
///                     assert_eq!("5g5f", best_move_sfen);
///                 }
///         _ => {}
///     }
///     Ok(())
/// }).unwrap();
/// handler.send_command(&GuiCommand::Usi).unwrap();
/// ```
#[derive(Debug)]
pub struct UsiEngineHandler {
    process: Child,
    reader: Option<EngineCommandReader<BufReader<ChildStdout>>>,
    writer: GuiCommandWriter<ChildStdin>,
}

impl Drop for UsiEngineHandler {
    fn drop(&mut self) {
        self.kill().unwrap();
    }
}
impl UsiEngineHandler {
    /// Spanws a new process of the specific USI engine.
    pub fn spawn<P: AsRef<OsStr>, Q: AsRef<Path>>(
        engine_path: P,
        working_dir: Q,
    ) -> Result<UsiEngineHandler, Error> {
        let mut process = Command::new(engine_path)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        let stdin = process.stdin.take().unwrap();
        let stdout = process.stdout.take().unwrap();

        Ok(UsiEngineHandler {
            process,
            reader: Some(EngineCommandReader::new(BufReader::new(stdout))),
            writer: GuiCommandWriter::new(stdin),
        })
    }

    /// Bootstrap the engine and returns a metadata such as an engine name and available options.
    /// Returns `Error::IllegalOperation` when called after `listen` method.
    pub fn prepare(&mut self) -> Result<EngineInfo, Error> {
        let reader = match &mut self.reader {
            Some(r) => Ok(r),
            None => Err(Error::IllegalOperation),
        }?;

        let mut info = EngineInfo::default();
        self.writer.send(&GuiCommand::Usi)?;

        loop {
            let output = reader.next_command()?;
            match output.response() {
                Some(EngineCommand::Id(IdParams::Name(name))) => {
                    info.name = name.to_string();
                }
                Some(EngineCommand::Option(OptionParams {
                    ref name,
                    ref value,
                })) => {
                    info.options.insert(
                        name.to_string(),
                        match value {
                            OptionKind::Check { default: Some(f) } => {
                                if *f { "true" } else { "false" }.to_string()
                            }
                            OptionKind::Spin {
                                default: Some(n), ..
                            } => n.to_string(),
                            OptionKind::Combo {
                                default: Some(s), ..
                            } => s.to_string(),
                            OptionKind::Button { default: Some(s) } => s.to_string(),
                            OptionKind::String { default: Some(s) } => s.to_string(),
                            OptionKind::Filename { default: Some(s) } => s.to_string(),
                            _ => String::new(),
                        },
                    );
                }
                Some(EngineCommand::UsiOk) => break,
                _ => {}
            }
        }

        Ok(info)
    }

    /// Sends a command to the engine.
    pub fn send_command(&mut self, command: &GuiCommand) -> Result<(), Error> {
        self.writer.send(command)
    }

    /// Terminates the engine.
    pub fn kill(&mut self) -> Result<(), Error> {
        self.writer.send(&GuiCommand::Quit)?;
        self.process.kill()?;
        Ok(())
    }

    /// Spanws a new thread to monitor outputs from the engine.
    /// `hook` will be called for each USI command received.
    /// `prepare` method can only be called before `listen` method.
    pub fn listen<F, E>(&mut self, mut hook: F) -> Result<(), Error>
    where
        F: FnMut(&EngineOutput) -> Result<(), E> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        let mut reader = self.reader.take().ok_or(Error::IllegalOperation)?;

        thread::spawn(move || -> Result<(), Error> {
            loop {
                match reader.next_command() {
                    Ok(output) => {
                        if let Err(e) = hook(&output) {
                            return Err(Error::HandlerError(Box::new(e)));
                        }
                    }
                    Err(Error::IllegalSyntax) => {
                        // Ignore illegal commands.
                        continue;
                    }
                    Err(err) => {
                        return Err(err);
                    }
                }
            }
        });

        Ok(())
    }
}
