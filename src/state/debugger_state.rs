// ─── Frame ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Frame {
    pub addr: u64,
    pub function: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

// ─── Breakpoint ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Breakpoint {
    pub id: u32,
    pub file: String,
    /// Línea real donde GDB colocó el breakpoint (puede diferir de la pedida:
    /// al pedir uno en la línea del nombre de una función, GDB lo reubica a la
    /// primera línea ejecutable del cuerpo).
    pub line: u32,
    /// Línea originalmente solicitada (de `original-location`), si se conoce.
    /// Permite que un click sobre esa línea quite el breakpoint aunque GDB lo
    /// haya movido a otra.
    pub requested_line: Option<u32>,
    pub enabled: bool,
    /// Expresión de condición GDB (`-c "<cond>"` / `-break-condition`), si el
    /// breakpoint es condicional. `None` = incondicional.
    pub condition: Option<String>,
    /// Mensaje de `^error` de GDB tras un intento fallido de fijar/editar la
    /// condición. Se limpia (`None`) en cualquier merge exitoso posterior.
    pub condition_error: Option<String>,
}

// ─── Variable (locals / watch) ────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub value: String,

    pub type_: String,
}

// ─── Register ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Register {
    pub number: u32,
    pub name: String,
    pub value: String, // hex: "0x00007fff..."
}

// ─── Disassembly ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct AsmLine {
    pub addr: u64,
    pub inst: String,
}

// ─── Stop reason ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum StopReason {
    BreakpointHit(u32),
    EndStepping,
    Signal(String),
    Unknown,
}

// ─── Pause state ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PauseState {
    pub thread_id: u32,
    pub frame: Frame,
    pub stack: Vec<Frame>,
    pub stop_reason: StopReason,
}

// ─── Program state ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum ProgramState {
    NoProgramLoaded,
    ProgramLoaded,
    Running,
    Paused,
    Exited { code: Option<i32> },
}

// ─── Persistent state ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct PersistentState {
    pub executable: Option<String>,
    pub breakpoints: Vec<Breakpoint>,
}

// ─── Top-level state ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct DebuggerState {
    pub program: ProgramState,
    pub pause: Option<PauseState>,
    pub locals: Vec<Variable>,
    pub register_names: Vec<String>,
    pub registers: Vec<Register>,
    pub disasm: Vec<AsmLine>,
    pub global_names: Vec<String>,
    pub globals: Vec<Variable>,
    pub struct_expr: String,
    pub struct_value: Option<String>,
    pub persistent: PersistentState,
}

// ─── Events ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum StateEvent {
    ProgramLoaded { executable: String },
    ProgramStarted,
    ProgramPaused { pause: PauseState },
    ProgramExited { code: Option<i32> },
    StackUpdated { frames: Vec<Frame> },
    BreakpointAdded { breakpoint: Breakpoint },
    BreakpointRemoved { id: u32 },
    BreakpointToggled { id: u32, enabled: bool },
    BreakpointConditionError { id: u32, message: String },
    LocalsUpdated { vars: Vec<Variable> },
    RegisterNamesReceived { names: Vec<String> },
    RegistersUpdated { registers: Vec<Register> },
    DisasmUpdated { lines: Vec<AsmLine> },
    GlobalNamesReceived { names: Vec<String> },
    GlobalValueUpdated { name: String, value: String },
    StructValueUpdated { expr: String, value: String },
}

#[derive(Clone, Debug)]
pub enum UiEvent {
    ConsoleOutput(String),
    GdbError(String),
}

#[derive(Clone, Debug)]
pub enum DebuggerEvent {
    State(StateEvent),
    Ui(UiEvent),
}

// ─── impl ────────────────────────────────────────────────────────────────────

impl DebuggerState {
    pub fn new() -> Self {
        Self {
            program: ProgramState::NoProgramLoaded,
            pause: None,
            locals: vec![],
            register_names: vec![],
            registers: vec![],
            disasm: vec![],
            global_names: vec![],
            globals: vec![],
            struct_expr: String::new(),
            struct_value: None,
            persistent: PersistentState {
                executable: None,
                breakpoints: vec![],
            },
        }
    }

    pub fn apply(&mut self, event: StateEvent) {
        match event {
            StateEvent::ProgramLoaded { executable } => {
                self.program = ProgramState::ProgramLoaded;
                self.persistent.executable = Some(executable);
                self.pause = None;
                self.locals = vec![];
                self.register_names = vec![];
                self.registers = vec![];
                self.disasm = vec![];
                self.global_names = vec![];
                self.globals = vec![];
                // New executable: symbols change, so a previously committed
                // expression may no longer resolve — reset both.
                self.struct_expr = String::new();
                self.struct_value = None;
            }

            StateEvent::ProgramStarted => {
                self.program = ProgramState::Running;
                self.pause = None;
                self.locals = vec![];
                // register_names are architecture-static — fetched once on load,
                // so we keep them across runs instead of wiping and refetching.
                self.registers = vec![];
                self.disasm = vec![];
                self.globals = vec![];
                // struct_expr survives run/pause/continue cycles so it is
                // auto-re-evaluated on the next pause; only the stale value clears.
                self.struct_value = None;
            }

            StateEvent::ProgramPaused { pause } => {
                self.program = ProgramState::Paused;
                self.pause = Some(pause);
            }

            StateEvent::StackUpdated { frames } => {
                // Llega justo después del *stopped (que solo trae el frame superior);
                // reemplaza el stack de un solo frame por el completo.
                if let Some(pause) = &mut self.pause {
                    if let Some(top) = frames.first() {
                        pause.frame = top.clone();
                    }
                    pause.stack = frames;
                }
            }

            StateEvent::ProgramExited { code } => {
                self.program = ProgramState::Exited { code };
                self.pause = None;
                self.locals = vec![];
                // Keep register_names — same executable, same architecture.
                self.registers = vec![];
                self.disasm = vec![];
                self.globals = vec![];
                self.struct_value = None;
            }

            StateEvent::BreakpointAdded { breakpoint } => {
                if let Some(existing) = self
                    .persistent
                    .breakpoints
                    .iter_mut()
                    .find(|b| b.id == breakpoint.id)
                {
                    *existing = breakpoint;
                } else {
                    self.persistent.breakpoints.push(breakpoint);
                }
            }

            StateEvent::BreakpointRemoved { id } => {
                self.persistent.breakpoints.retain(|b| b.id != id);
            }

            StateEvent::BreakpointToggled { id, enabled } => {
                if let Some(bp) = self.persistent.breakpoints.iter_mut().find(|b| b.id == id) {
                    bp.enabled = enabled;
                }
            }

            StateEvent::BreakpointConditionError { id, message } => {
                if let Some(bp) = self.persistent.breakpoints.iter_mut().find(|b| b.id == id) {
                    bp.condition_error = Some(message);
                }
            }

            StateEvent::LocalsUpdated { vars } => self.locals = vars,
            StateEvent::RegisterNamesReceived { names } => self.register_names = names,
            StateEvent::RegistersUpdated { registers } => self.registers = registers,
            StateEvent::DisasmUpdated { lines } => self.disasm = lines,

            StateEvent::GlobalNamesReceived { names } => self.global_names = names,
            StateEvent::GlobalValueUpdated { name, value } => {
                if let Some(existing) = self.globals.iter_mut().find(|v| v.name == name) {
                    existing.value = value;
                } else {
                    self.globals.push(Variable {
                        name,
                        value,
                        type_: String::new(),
                    });
                }
            }
            StateEvent::StructValueUpdated { expr, value } => {
                if expr == self.struct_expr {
                    self.struct_value = Some(value);
                }
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    pub fn is_paused(&self) -> bool {
        matches!(self.program, ProgramState::Paused)
    }

    pub fn is_running(&self) -> bool {
        matches!(self.program, ProgramState::Running)
    }

    pub fn current_file(&self) -> Option<&str> {
        self.pause.as_ref()?.frame.file.as_deref()
    }

    pub fn current_line(&self) -> Option<u32> {
        self.pause.as_ref()?.frame.line
    }

    pub fn current_function(&self) -> Option<&str> {
        Some(self.pause.as_ref()?.frame.function.as_str())
    }

    pub fn current_addr(&self) -> Option<u64> {
        Some(self.pause.as_ref()?.frame.addr)
    }

    /// Breakpoint que un click en `line` debe alternar. Coincide con la línea
    /// real donde GDB lo puso o con la línea originalmente solicitada, de modo
    /// que se pueda quitar tanto desde la línea del nombre de una función como
    /// desde la línea ejecutable a la que GDB lo reubicó.
    pub fn breakpoint_at(&self, file: &str, line: u32) -> Option<&Breakpoint> {
        self.persistent.breakpoints.iter().find(|b| {
            same_file(&b.file, file) && (b.line == line || b.requested_line == Some(line))
        })
    }

    /// ¿Debe dibujarse el marcador de breakpoint en esta línea? Solo en la línea
    /// real donde GDB detiene la ejecución, no en la solicitada (que GDB pudo
    /// reubicar), para no mostrar un punto fantasma en dos líneas.
    pub fn has_breakpoint_marker(&self, file: &str, line: u32) -> bool {
        self.persistent
            .breakpoints
            .iter()
            .any(|b| b.line == line && same_file(&b.file, file))
    }

    /// Comprueba si `candidate` es redundante: ya existe otro breakpoint (con
    /// distinto id) en la misma ubicación *resuelta* (archivo + línea real).
    ///
    /// Ocurre porque GDB reubica los breakpoints pedidos en líneas no ejecutables
    /// a la siguiente línea real: p.ej. pedir la línea 11 y la 12 puede acabar en
    /// la misma línea 12 con dos ids distintos. Devuelve `true` para descartar el
    /// duplicado y quedarnos con uno solo.
    pub fn is_duplicate_breakpoint(&self, candidate: &Breakpoint) -> bool {
        self.persistent.breakpoints.iter().any(|b| {
            b.id != candidate.id && b.line == candidate.line && same_file(&b.file, &candidate.file)
        })
    }
}

/// Compara dos rutas de archivo de forma tolerante.
///
/// GDB puede reportar la ruta de un breakpoint de forma distinta a la del frame
/// actual (absoluta vs relativa, con o sin prefijos como `./`). Consideramos que
/// dos rutas apuntan al mismo archivo si son iguales o si la más corta es un
/// sufijo —por componentes— de la más larga. Comparar por componentes evita
/// falsos positivos como `foobar.c` vs `bar.c`.
fn same_file(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }

    let a_comps: Vec<_> = std::path::Path::new(a).components().collect();
    let b_comps: Vec<_> = std::path::Path::new(b).components().collect();
    let n = a_comps.len().min(b_comps.len());
    if n == 0 {
        return false;
    }

    a_comps[a_comps.len() - n..] == b_comps[b_comps.len() - n..]
}

impl Default for DebuggerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bp(id: u32, file: &str, line: u32) -> Breakpoint {
        Breakpoint {
            id,
            file: file.into(),
            line,
            requested_line: None,
            enabled: true,
            condition: None,
            condition_error: None,
        }
    }

    #[test]
    fn breakpoint_has_condition_and_condition_error_fields() {
        let b = Breakpoint {
            id: 1,
            file: "a.c".into(),
            line: 5,
            requested_line: None,
            enabled: true,
            condition: Some("x == 1".into()),
            condition_error: Some("No symbol \"x\"".into()),
        };
        assert_eq!(b.condition, Some("x == 1".to_string()));
        assert_eq!(b.condition_error, Some("No symbol \"x\"".to_string()));
    }

    // La respuesta `=breakpoint-modified` re-parsea la fila completa (replace-by-id):
    // debe actualizar `condition` con el nuevo valor y limpiar cualquier
    // `condition_error` previo, ya que un merge exitoso implica que GDB aceptó.
    #[test]
    fn breakpoint_added_merge_updates_condition_and_clears_error() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: None,
            enabled: true,
            condition: None,
            condition_error: Some("previous error".into()),
        });

        state.apply(StateEvent::BreakpointAdded {
            breakpoint: Breakpoint {
                id: 1,
                file: "/tmp/example.c".into(),
                line: 7,
                requested_line: None,
                enabled: true,
                condition: Some("count > 3".into()),
                condition_error: None,
            },
        });

        let bp = state
            .persistent
            .breakpoints
            .iter()
            .find(|b| b.id == 1)
            .expect("breakpoint 1 must still exist");
        assert_eq!(bp.condition, Some("count > 3".to_string()));
        assert_eq!(bp.condition_error, None);
    }

    #[test]
    fn breakpoint_condition_error_sets_error_leaves_condition_untouched() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: None,
            enabled: true,
            condition: Some("x == 1".into()),
            condition_error: None,
        });

        state.apply(StateEvent::BreakpointConditionError {
            id: 1,
            message: "No symbol \"unknown_symbol_xyz\" in current context.".into(),
        });

        let bp = state
            .persistent
            .breakpoints
            .iter()
            .find(|b| b.id == 1)
            .expect("breakpoint 1 must still exist");
        assert_eq!(bp.condition, Some("x == 1".to_string()));
        assert_eq!(
            bp.condition_error,
            Some("No symbol \"unknown_symbol_xyz\" in current context.".to_string())
        );
    }

    #[test]
    fn same_file_matches_absolute_and_relative() {
        assert!(same_file("/tmp/example.c", "/tmp/example.c"));
        assert!(same_file("/tmp/example.c", "example.c"));
        assert!(same_file("example.c", "/tmp/example.c"));
        assert!(same_file("/home/user/src/example.c", "src/example.c"));
        assert!(same_file("./example.c", "example.c"));
    }

    #[test]
    fn same_file_rejects_different_files() {
        assert!(!same_file("/tmp/foobar.c", "bar.c"));
        assert!(!same_file("/tmp/a/example.c", "/tmp/b/example.c"));
        assert!(!same_file("main.c", "example.c"));
    }

    #[test]
    fn struct_value_updated_matching_expr_sets_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct.field".into();

        state.apply(StateEvent::StructValueUpdated {
            expr: "my_struct.field".into(),
            value: "{a = 1, b = 2}".into(),
        });

        assert_eq!(state.struct_value, Some("{a = 1, b = 2}".to_string()));
    }

    #[test]
    fn struct_value_updated_stale_expr_is_dropped() {
        let mut state = DebuggerState::new();
        state.struct_expr = "current_expr".into();
        state.struct_value = Some("stale value".into());

        state.apply(StateEvent::StructValueUpdated {
            expr: "old_expr".into(),
            value: "should not apply".into(),
        });

        assert_eq!(state.struct_value, Some("stale value".to_string()));
    }

    #[test]
    fn program_loaded_clears_struct_expr_and_struct_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct".into();
        state.struct_value = Some("{a = 1}".into());

        state.apply(StateEvent::ProgramLoaded {
            executable: "a.out".into(),
        });

        assert_eq!(state.struct_expr, "");
        assert_eq!(state.struct_value, None);
    }

    #[test]
    fn program_started_clears_only_struct_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct".into();
        state.struct_value = Some("{a = 1}".into());

        state.apply(StateEvent::ProgramStarted);

        assert_eq!(state.struct_expr, "my_struct");
        assert_eq!(state.struct_value, None);
    }

    #[test]
    fn program_exited_clears_only_struct_value() {
        let mut state = DebuggerState::new();
        state.struct_expr = "my_struct".into();
        state.struct_value = Some("{a = 1}".into());

        state.apply(StateEvent::ProgramExited { code: Some(0) });

        assert_eq!(state.struct_expr, "my_struct");
        assert_eq!(state.struct_value, None);
    }

    // Reproduce el bug: GDB guarda el breakpoint con ruta absoluta, pero el
    // click consulta con la ruta tal cual la conoce la UI. El toggle debe
    // encontrar el breakpoint existente en vez de crear un duplicado.
    #[test]
    fn breakpoint_at_finds_bp_despite_path_form() {
        let mut state = DebuggerState::new();
        state
            .persistent
            .breakpoints
            .push(bp(1, "/tmp/example.c", 8));

        assert!(state.breakpoint_at("/tmp/example.c", 8).is_some());
        assert!(state.breakpoint_at("example.c", 8).is_some());
        assert!(state.breakpoint_at("/tmp/example.c", 9).is_none());
    }

    // GDB reubica un breakpoint pedido en la línea del nombre de la función
    // (línea 6) a la primera línea ejecutable (línea 7). Un nuevo click sobre la
    // línea 6 debe encontrarlo (vía requested_line) y quitarlo, no duplicarlo.
    #[test]
    fn breakpoint_at_matches_requested_and_resolved_line() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: Some(6),
            enabled: true,
            condition: None,
            condition_error: None,
        });

        // Click sobre la línea real donde GDB lo puso.
        assert!(state.breakpoint_at("example.c", 7).is_some());
        // Click sobre la línea del nombre de la función (la pedida).
        assert!(state.breakpoint_at("example.c", 6).is_some());
        // Una línea sin relación no coincide.
        assert!(state.breakpoint_at("example.c", 5).is_none());
    }

    // El marcador visual solo debe aparecer en la línea real (7), no en la
    // solicitada (6), aunque el toggle sí funcione desde ambas.
    #[test]
    fn marker_only_on_resolved_line() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 7,
            requested_line: Some(6),
            enabled: true,
            condition: None,
            condition_error: None,
        });

        assert!(state.has_breakpoint_marker("example.c", 7));
        assert!(!state.has_breakpoint_marker("example.c", 6));
    }

    // Pedir la línea 11 y la 12 puede resolver ambas a la línea 12 con ids
    // distintos: el segundo es un duplicado que debe descartarse.
    #[test]
    fn detects_duplicate_at_resolved_line() {
        let mut state = DebuggerState::new();
        state.persistent.breakpoints.push(Breakpoint {
            id: 1,
            file: "/tmp/example.c".into(),
            line: 12,
            requested_line: Some(11),
            enabled: true,
            condition: None,
            condition_error: None,
        });

        // Mismo archivo+línea resuelta, id distinto → duplicado.
        let candidate = Breakpoint {
            id: 2,
            file: "/tmp/example.c".into(),
            line: 12,
            requested_line: Some(12),
            enabled: true,
            condition: None,
            condition_error: None,
        };
        assert!(state.is_duplicate_breakpoint(&candidate));

        // Distinta línea resuelta → no es duplicado.
        let other = Breakpoint {
            id: 3,
            file: "/tmp/example.c".into(),
            line: 8,
            requested_line: Some(8),
            enabled: true,
            condition: None,
            condition_error: None,
        };
        assert!(!state.is_duplicate_breakpoint(&other));

        // El mismo id (re-emisión del propio breakpoint) no cuenta como duplicado.
        assert!(!state.is_duplicate_breakpoint(&bp(1, "/tmp/example.c", 12)));
    }
}
