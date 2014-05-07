// Copyright 2012-2013 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.


use back::target_strs;
use back;
use back::link::OutputType;
use driver::driver::host_triple;
use front;
use metadata::filesearch;
use metadata;
use middle::lint;
use util::nodemap::NodeMap;

use syntax::attr::AttrMetaMethods;
use syntax::ast::NodeId;
use syntax::ast::{IntTy, UintTy};
use syntax::codemap::Span;
use syntax::diagnostic;
use syntax::parse::ParseSess;
use syntax::{abi, ast, codemap};

use std::cell::{Cell, RefCell};
use collections::HashSet;
use std::from_str::FromStr;

pub struct Config {
    pub os: abi::Os,
    pub arch: abi::Architecture,
    pub target_strs: target_strs::t,
    pub int_type: IntTy,
    pub uint_type: UintTy,
}

macro_rules! debugging_opts(
    ([ $opt:ident ] $cnt:expr ) => (
        pub static $opt: u64 = 1 << $cnt;
    );
    ([ $opt:ident, $($rest:ident),* ] $cnt:expr ) => (
        pub static $opt: u64 = 1 << $cnt;
        debugging_opts!([ $($rest),* ] $cnt + 1)
    )
)

debugging_opts!(
    [
        VERBOSE,
        TIME_PASSES,
        COUNT_LLVM_INSNS,
        TIME_LLVM_PASSES,
        TRANS_STATS,
        ASM_COMMENTS,
        NO_VERIFY,
        BORROWCK_STATS,
        NO_LANDING_PADS,
        DEBUG_LLVM,
        SHOW_SPAN,
        COUNT_TYPE_SIZES,
        META_STATS,
        NO_OPT,
        GC,
        PRINT_LINK_ARGS,
        PRINT_LLVM_PASSES,
        LTO,
        AST_JSON,
        AST_JSON_NOEXPAND,
        LS
    ]
    0
)

pub fn debugging_opts_map() -> Vec<(&'static str, &'static str, u64)> {
    vec!(("verbose", "in general, enable more debug printouts", VERBOSE),
     ("time-passes", "measure time of each rustc pass", TIME_PASSES),
     ("count-llvm-insns", "count where LLVM \
                           instrs originate", COUNT_LLVM_INSNS),
     ("time-llvm-passes", "measure time of each LLVM pass",
      TIME_LLVM_PASSES),
     ("trans-stats", "gather trans statistics", TRANS_STATS),
     ("asm-comments", "generate comments into the assembly (may change behavior)",
      ASM_COMMENTS),
     ("no-verify", "skip LLVM verification", NO_VERIFY),
     ("borrowck-stats", "gather borrowck statistics",  BORROWCK_STATS),
     ("no-landing-pads", "omit landing pads for unwinding",
      NO_LANDING_PADS),
     ("debug-llvm", "enable debug output from LLVM", DEBUG_LLVM),
     ("show-span", "show spans for compiler debugging", SHOW_SPAN),
     ("count-type-sizes", "count the sizes of aggregate types",
      COUNT_TYPE_SIZES),
     ("meta-stats", "gather metadata statistics", META_STATS),
     ("no-opt", "do not optimize, even if -O is passed", NO_OPT),
     ("print-link-args", "Print the arguments passed to the linker",
      PRINT_LINK_ARGS),
     ("gc", "Garbage collect shared data (experimental)", GC),
     ("print-llvm-passes",
      "Prints the llvm optimization passes being run",
      PRINT_LLVM_PASSES),
     ("lto", "Perform LLVM link-time optimizations", LTO),
     ("ast-json", "Print the AST as JSON and halt", AST_JSON),
     ("ast-json-noexpand", "Print the pre-expansion AST as JSON and halt", AST_JSON_NOEXPAND),
     ("ls", "List the symbols defined by a library crate", LS))
}

#[deriving(Clone, Eq)]
pub enum OptLevel {
    No, // -O0
    Less, // -O1
    Default, // -O2
    Aggressive // -O3
}

#[deriving(Clone, Eq)]
pub enum DebugInfoLevel {
    NoDebugInfo,
    LimitedDebugInfo,
    FullDebugInfo,
}
#[deriving(Clone, Eq)]
pub enum NaClFlavor_ {
    NaClFlavor,
    PNaClFlavor,
    EmscriptenFlavor,
}
impl FromStr for NaClFlavor_ {
    fn from_str(s: &str) -> Option<NaClFlavor_> {
        match s {
            "nacl" => Some(NaClFlavor),
            "pnacl" => Some(PNaClFlavor),
            "emscripten" => Some(EmscriptenFlavor),
            _ => None,
        }
    }
}

#[deriving(Clone)]
pub struct Options {
    // The crate config requested for the session, which may be combined
    // with additional crate configurations during the compile process
    pub crate_types: Vec<CrateType> ,

    pub gc: bool,
    pub optimize: OptLevel,
    pub debuginfo: DebugInfoLevel,
    pub lint_opts: Vec<(lint::Lint, lint::level)> ,
    pub output_types: Vec<back::link::OutputType> ,
    // This was mutable for rustpkg, which updates search paths based on the
    // parsed code. It remains mutable in case its replacements wants to use
    // this.
    pub addl_lib_search_paths: RefCell<HashSet<Path>>,
    pub maybe_sysroot: Option<Path>,
    pub target_triple: ~str,
    // User-specified cfg meta items. The compiler itself will add additional
    // items to the crate config, and during parsing the entire crate config
    // will be added to the crate AST node.  This should not be used for
    // anything except building the full crate config prior to parsing.
    pub cfg: ast::CrateConfig,
    pub test: bool,
    pub parse_only: bool,
    pub no_trans: bool,
    pub no_analysis: bool,
    pub debugging_opts: u64,
    /// Whether to write dependency files. It's (enabled, optional filename).
    pub write_dependency_info: (bool, Option<Path>),
    /// Crate id-related things to maybe print. It's (crate_id, crate_name, crate_file_name).
    pub print_metas: (bool, bool, bool),
    pub cg: CodegenOptions,
}

// The type of entry function, so
// users can have their own entry
// functions that don't start a
// scheduler
#[deriving(Eq)]
pub enum EntryFnType {
    EntryMain,
    EntryStart,
    EntryNone,
}

#[deriving(Eq, Ord, Clone, TotalOrd, TotalEq)]
pub enum CrateType {
    CrateTypeExecutable,
    CrateTypeDylib,
    CrateTypeRlib,
    CrateTypeStaticlib,
}

pub struct Session {
    pub targ_cfg: Config,
    pub opts: Options,
    pub cstore: metadata::cstore::CStore,
    pub parse_sess: ParseSess,
    // For a library crate, this is always none
    pub entry_fn: RefCell<Option<(NodeId, codemap::Span)>>,
    pub entry_type: Cell<Option<EntryFnType>>,
    pub macro_registrar_fn: Cell<Option<ast::NodeId>>,
    pub default_sysroot: Option<Path>,
    // The name of the root source file of the crate, in the local file system. The path is always
    // expected to be absolute. `None` means that there is no source file.
    pub local_crate_source_file: Option<Path>,
    pub working_dir: Path,
    pub lints: RefCell<NodeMap<Vec<(lint::Lint, codemap::Span, ~str)>>>,
    pub node_id: Cell<ast::NodeId>,
    pub crate_types: RefCell<Vec<CrateType>>,
    pub features: front::feature_gate::Features,

    /// The maximum recursion limit for potentially infinitely recursive
    /// operations such as auto-dereference and monomorphization.
    pub recursion_limit: Cell<uint>,
}

impl Session {
    pub fn span_fatal(&self, sp: Span, msg: &str) -> ! {
        self.diagnostic().span_fatal(sp, msg)
    }
    pub fn fatal(&self, msg: &str) -> ! {
        self.diagnostic().handler().fatal(msg)
    }
    pub fn span_err(&self, sp: Span, msg: &str) {
        self.diagnostic().span_err(sp, msg)
    }
    pub fn err(&self, msg: &str) {
        self.diagnostic().handler().err(msg)
    }
    pub fn err_count(&self) -> uint {
        self.diagnostic().handler().err_count()
    }
    pub fn has_errors(&self) -> bool {
        self.diagnostic().handler().has_errors()
    }
    pub fn abort_if_errors(&self) {
        self.diagnostic().handler().abort_if_errors()
    }
    pub fn span_warn(&self, sp: Span, msg: &str) {
        self.diagnostic().span_warn(sp, msg)
    }
    pub fn warn(&self, msg: &str) {
        self.diagnostic().handler().warn(msg)
    }
    pub fn span_note(&self, sp: Span, msg: &str) {
        self.diagnostic().span_note(sp, msg)
    }
    pub fn span_end_note(&self, sp: Span, msg: &str) {
        self.diagnostic().span_end_note(sp, msg)
    }
    pub fn fileline_note(&self, sp: Span, msg: &str) {
        self.diagnostic().fileline_note(sp, msg)
    }
    pub fn note(&self, msg: &str) {
        self.diagnostic().handler().note(msg)
    }
    pub fn span_bug(&self, sp: Span, msg: &str) -> ! {
        self.diagnostic().span_bug(sp, msg)
    }
    pub fn bug(&self, msg: &str) -> ! {
        self.diagnostic().handler().bug(msg)
    }
    pub fn span_unimpl(&self, sp: Span, msg: &str) -> ! {
        self.diagnostic().span_unimpl(sp, msg)
    }
    pub fn unimpl(&self, msg: &str) -> ! {
        self.diagnostic().handler().unimpl(msg)
    }
    pub fn add_lint(&self,
                    lint: lint::Lint,
                    id: ast::NodeId,
                    sp: Span,
                    msg: ~str) {
        let mut lints = self.lints.borrow_mut();
        match lints.find_mut(&id) {
            Some(arr) => { arr.push((lint, sp, msg)); return; }
            None => {}
        }
        lints.insert(id, vec!((lint, sp, msg)));
    }
    pub fn next_node_id(&self) -> ast::NodeId {
        self.reserve_node_ids(1)
    }
    pub fn reserve_node_ids(&self, count: ast::NodeId) -> ast::NodeId {
        let v = self.node_id.get();

        match v.checked_add(&count) {
            Some(next) => { self.node_id.set(next); }
            None => self.bug("Input too large, ran out of node ids!")
        }

        v
    }
    pub fn diagnostic<'a>(&'a self) -> &'a diagnostic::SpanHandler {
        &self.parse_sess.span_diagnostic
    }
    pub fn debugging_opt(&self, opt: u64) -> bool {
        (self.opts.debugging_opts & opt) != 0
    }
    pub fn codemap<'a>(&'a self) -> &'a codemap::CodeMap {
        &self.parse_sess.span_diagnostic.cm
    }
    // This exists to help with refactoring to eliminate impossible
    // cases later on
    pub fn impossible_case(&self, sp: Span, msg: &str) -> ! {
        self.span_bug(sp, format!("impossible case reached: {}", msg));
    }
    pub fn verbose(&self) -> bool { self.debugging_opt(VERBOSE) }
    pub fn time_passes(&self) -> bool { self.debugging_opt(TIME_PASSES) }
    pub fn count_llvm_insns(&self) -> bool {
        self.debugging_opt(COUNT_LLVM_INSNS)
    }
    pub fn count_type_sizes(&self) -> bool {
        self.debugging_opt(COUNT_TYPE_SIZES)
    }
    pub fn time_llvm_passes(&self) -> bool {
        self.debugging_opt(TIME_LLVM_PASSES)
    }
    pub fn trans_stats(&self) -> bool { self.debugging_opt(TRANS_STATS) }
    pub fn meta_stats(&self) -> bool { self.debugging_opt(META_STATS) }
    pub fn asm_comments(&self) -> bool { self.debugging_opt(ASM_COMMENTS) }
    pub fn no_verify(&self) -> bool { self.debugging_opt(NO_VERIFY) }
    pub fn borrowck_stats(&self) -> bool { self.debugging_opt(BORROWCK_STATS) }
    pub fn print_llvm_passes(&self) -> bool {
        self.debugging_opt(PRINT_LLVM_PASSES)
    }
    pub fn lto(&self) -> bool {
        self.debugging_opt(LTO)
    }
    pub fn no_landing_pads(&self) -> bool {
        self.debugging_opt(NO_LANDING_PADS)
    }
    pub fn show_span(&self) -> bool {
        self.debugging_opt(SHOW_SPAN)
    }
    pub fn sysroot<'a>(&'a self) -> &'a Path {
        match self.opts.maybe_sysroot {
            Some (ref sysroot) => sysroot,
            None => self.default_sysroot.as_ref()
                        .expect("missing sysroot and default_sysroot in Session")
        }
    }
    pub fn target_filesearch<'a>(&'a self) -> filesearch::FileSearch<'a> {
        filesearch::FileSearch::new(
            self.sysroot(),
            self.opts.target_triple,
            &self.opts.addl_lib_search_paths)
    }
    pub fn host_filesearch<'a>(&'a self) -> filesearch::FileSearch<'a> {
        filesearch::FileSearch::new(
            self.sysroot(),
            host_triple(),
            &self.opts.addl_lib_search_paths)
    }

    pub fn no_morestack(&self) -> bool {
        (match self.opts.cg.nacl_flavor {
            None => false,
            Some(NaClFlavor) => false,
            Some(_) => true,
        }) && self.targ_cfg.target_strs.target_triple == "le32-unknown-nacl".to_owned()
    }
    // true if we should feed our generated ll to our "linker"
    // used for Emscripten && PNaCl
    pub fn linker_eats_ll(&self) -> bool {
        (match self.opts.cg.nacl_flavor {
            None => false,
            Some(NaClFlavor) => false,
            Some(_) => true,
        }) && self.targ_cfg.target_strs.target_triple == "le32-unknown-nacl".to_owned()
    }

    pub fn get_nacl_tool_path(&self,
                              em_suffix: &str,
                              nacl_suffix: &str,
                              pnacl_suffix: &str) -> ~str {
        let toolchain = self.expect_cross_path();
        match self.opts.cg.nacl_flavor {
            Some(EmscriptenFlavor) => toolchain.join(em_suffix),
            Some(_) => {
                let (arch_libc, prefix) = match self.targ_cfg.arch {
                    abi::X86 =>    ("x86_newlib", "i686-nacl-"),
                    abi::X86_64 => ("x86_newlib", "x86_64-nacl-"),
                    abi::Arm =>    ("arm_newlib", "arm-nacl-"),
                    abi::Le32 =>   ("pnacl",      "pnacl-"),
                    abi::Mips =>   self.fatal("PNaCl/NaCl can't target the Mips arch"),
                };
                let post_toolchain = format!("{}_{}",
                                             get_os_for_nacl_toolchain(self),
                                             arch_libc);
                let tool_name = format!("{}{}",
                                        prefix,
                                        match self.opts.cg.nacl_flavor {
                                            Some(EmscriptenFlavor) | None => unreachable!(),
                                            Some(NaClFlavor) => nacl_suffix,
                                            Some(PNaClFlavor) => pnacl_suffix,
                                        });
                toolchain.join_many(["toolchain".to_owned(),
                                     post_toolchain,
                                     "bin".to_owned(),
                                     tool_name])
            }
            None => self.bug("get_nacl_tool_path called without NaCl flavor"),
        }.as_str().unwrap().to_owned()
    }

    pub fn expect_cross_path(&self) -> Path {
        match self.opts.cg.cross_path {
            None => self.fatal("need cross path (-C cross-path) \
                                for this target"),
            Some(ref p) => Path::new(p.clone()),
        }
    }

    pub fn pnacl_toolchain(&self) -> Path {
        let tc = self.expect_cross_path();
        tc.join_many(["toolchain".to_owned(),
                      format!("{}_pnacl", get_os_for_nacl_toolchain(self))])
    }

    /// Shortcut to test if we need to do special things because we are targeting PNaCl.
    pub fn targeting_pnacl(&self) -> bool {
        (match self.opts.cg.nacl_flavor {
            Some(PNaClFlavor) => true,
            _ => false,
        }) && self.targ_cfg.target_strs.target_triple == "le32-unknown-nacl".to_owned()
    }
    pub fn would_use_ppapi(&self) -> bool {
        match self.opts.cg.nacl_flavor {
            Some(EmscriptenFlavor) | None => false,
            _ => true,
        }
    }

    // Emits a fatal error if path is not writeable.
    pub fn check_writeable_output(&self, path: &Path, name: &str) {
        use std::io;
        let is_writeable = match path.stat() {
            Err(..) => true,
            Ok(m) => m.perm & io::UserWrite == io::UserWrite
        };
        if !is_writeable {
            self.fatal(format!("`{}` file `{}` is not writeable -- check it's permissions.",
                               name, path.display()));
        }
    }

    // checks if we're saving temps or if we're emitting the specified type.
    // If neither, the file is unlinked from the filesystem.
    pub fn remove_temp(&self, path: &Path, t: OutputType) {
        use std::io::fs;
        if self.opts.cg.save_temps ||
            self.opts.output_types.contains(&t) {
            return;
        }
        match fs::unlink(path) {
            Ok(..) => {}
            Err(e) => {
                // strictly speaking, this isn't a fatal error.
                self.warn(format!("failed to remove `{}`: `{}`", path.display(), e));
            }
        }
    }
    // Create a 'temp' if we're either saving all temps, or --emit-ing that
    // output type.
    pub fn create_temp(&self, t: OutputType, f: ||) {
        if self.opts.cg.save_temps ||
            self.opts.output_types.contains(&t) {
            return;
        }
        f()
    }
}

/// Some reasonable defaults
pub fn basic_options() -> Options {
    Options {
        crate_types: Vec::new(),
        gc: false,
        optimize: No,
        debuginfo: NoDebugInfo,
        lint_opts: Vec::new(),
        output_types: Vec::new(),
        addl_lib_search_paths: RefCell::new(HashSet::new()),
        maybe_sysroot: None,
        target_triple: host_triple().to_owned(),
        cfg: Vec::new(),
        test: false,
        parse_only: false,
        no_trans: false,
        no_analysis: false,
        debugging_opts: 0,
        write_dependency_info: (false, None),
        print_metas: (false, false, false),
        cg: basic_codegen_options(),
    }
}

/// Declare a macro that will define all CodegenOptions fields and parsers all
/// at once. The goal of this macro is to define an interface that can be
/// programmatically used by the option parser in order to initialize the struct
/// without hardcoding field names all over the place.
///
/// The goal is to invoke this macro once with the correct fields, and then this
/// macro generates all necessary code. The main gotcha of this macro is the
/// cgsetters module which is a bunch of generated code to parse an option into
/// its respective field in the struct. There are a few hand-written parsers for
/// parsing specific types of values in this module.
macro_rules! cgoptions(
    ($($opt:ident : $t:ty = ($init:expr, $parse:ident, $desc:expr)),* ,) =>
(
    #[deriving(Clone)]
    pub struct CodegenOptions { $(pub $opt: $t),* }

    pub fn basic_codegen_options() -> CodegenOptions {
        CodegenOptions { $($opt: $init),* }
    }

    pub type CodegenSetter = fn(&mut CodegenOptions, v: Option<&str>) -> bool;
    pub static CG_OPTIONS: &'static [(&'static str, CodegenSetter,
                                      &'static str)] =
        &[ $( (stringify!($opt), cgsetters::$opt, $desc) ),* ];

    mod cgsetters {
        use super::CodegenOptions;
        use std::from_str::FromStr;

        $(
            pub fn $opt(cg: &mut CodegenOptions, v: Option<&str>) -> bool {
                $parse(&mut cg.$opt, v)
            }
        )*

        fn parse_bool(slot: &mut bool, v: Option<&str>) -> bool {
            match v {
                Some(..) => false,
                None => { *slot = true; true }
            }
        }

        fn parse_opt_string(slot: &mut Option<~str>, v: Option<&str>) -> bool {
            match v {
                Some(s) => { *slot = Some(s.to_owned()); true },
                None => false,
            }
        }

        fn parse_string(slot: &mut ~str, v: Option<&str>) -> bool {
            match v {
                Some(s) => { *slot = s.to_owned(); true },
                None => false,
            }
        }

        fn parse_list(slot: &mut Vec<~str>, v: Option<&str>)
                      -> bool {
            match v {
                Some(s) => {
                    for s in s.words() {
                        slot.push(s.to_owned());
                    }
                    true
                },
                None => false,
            }
        }
        fn parse_from_str<T: FromStr>(slot: &mut Option<T>, v: Option<&str>) -> bool {
            *slot = v.and_then(|s| from_str(s) );
            slot.is_some()
        }
    }
) )

cgoptions!(
    ar: Option<~str> = (None, parse_opt_string,
        "tool to assemble archives with"),
    linker: Option<~str> = (None, parse_opt_string,
        "system linker to link outputs with"),
    link_args: Vec<~str> = (Vec::new(), parse_list,
        "extra arguments to pass to the linker (space separated)"),
    target_cpu: ~str = ("generic".to_owned(), parse_string,
        "select target processor (llc -mcpu=help for details)"),
    target_feature: ~str = ("".to_owned(), parse_string,
        "target specific attributes (llc -mattr=help for details)"),
    passes: Vec<~str> = (Vec::new(), parse_list,
        "a list of extra LLVM passes to run (space separated)"),
    llvm_args: Vec<~str> = (Vec::new(), parse_list,
        "a list of arguments to pass to llvm (space separated)"),
    save_temps: bool = (false, parse_bool,
        "save all temporary output files during compilation"),
    cross_path: Option<~str> = (None, parse_opt_string,
        "the path to the Android NDK"),
    no_rpath: bool = (false, parse_bool,
        "disables setting the rpath in libs/exes"),
    no_prepopulate_passes: bool = (false, parse_bool,
        "don't pre-populate the pass manager with a list of passes"),
    no_vectorize_loops: bool = (false, parse_bool,
        "don't run the loop vectorization optimization passes"),
    no_vectorize_slp: bool = (false, parse_bool,
        "don't run LLVM's SLP vectorization pass"),
    soft_float: bool = (false, parse_bool,
        "generate software floating point library calls"),
    prefer_dynamic: bool = (false, parse_bool,
        "prefer dynamic linking to static linking"),
    no_integrated_as: bool = (false, parse_bool,
        "use an external assembler rather than LLVM's integrated one"),
    relocation_model: ~str = ("pic".to_owned(), parse_string,
         "choose the relocation model to use (llc -relocation-model for details)"),
    nacl_flavor: Option<NaClFlavor_> = (None, parse_from_str,
        "use with =pnacl, =nacl, or =emscripten.
         Only applicable when coupled with a PNaCl, NaCl, or Emscripten cross"),
    extra_bitcode: Vec<~str> =
        (Vec::new(),
         parse_list,
         "a list of bitcode files to include for linking (PNaCl bin output only)"),
    stable_pexe: bool =
        (false,
         parse_bool,
         "write finalized, stable PNaCl bitcode. PNaCl binaries only. Don't use \
          if you'd like debugging info."),
)

// Seems out of place, but it uses session, so I'm putting it here
pub fn expect<T:Clone>(sess: &Session, opt: Option<T>, msg: || -> ~str) -> T {
    diagnostic::expect(sess.diagnostic(), opt, msg)
}

pub fn default_lib_output() -> CrateType {
    CrateTypeRlib
}

pub fn collect_crate_types(session: &Session,
                           attrs: &[ast::Attribute]) -> Vec<CrateType> {
    // If we're generating a test executable, then ignore all other output
    // styles at all other locations
    if session.opts.test {
        return vec!(CrateTypeExecutable)
    }

    // Only check command line flags if present. If no types are specified by
    // command line, then reuse the empty `base` Vec to hold the types that
    // will be found in crate attributes.
    let mut base = session.opts.crate_types.clone();
    if base.len() > 0 {
        return base
    } else {
        let iter = attrs.iter().filter_map(|a| {
            if a.name().equiv(&("crate_type")) {
                match a.value_str() {
                    Some(ref n) if n.equiv(&("rlib")) => Some(CrateTypeRlib),
                    Some(ref n) if n.equiv(&("dylib")) => Some(CrateTypeDylib),
                    Some(ref n) if n.equiv(&("lib")) => {
                        Some(default_lib_output())
                    }
                    Some(ref n) if n.equiv(&("staticlib")) => {
                        Some(CrateTypeStaticlib)
                    }
                    Some(ref n) if n.equiv(&("bin")) => Some(CrateTypeExecutable),
                    Some(_) => {
                        session.add_lint(lint::UnknownCrateType,
                                         ast::CRATE_NODE_ID,
                                         a.span,
                                         "invalid `crate_type` value".to_owned());
                        None
                    }
                    _ => {
                        session.add_lint(lint::UnknownCrateType, ast::CRATE_NODE_ID,
                                        a.span, "`crate_type` requires a value".to_owned());
                        None
                    }
                }
            } else {
                None
            }
        });
        base.extend(iter);
        if base.len() == 0 {
            base.push(CrateTypeExecutable);
        }
        base.as_mut_slice().sort();
        base.dedup();
        return base;
    }
}

pub fn sess_os_to_meta_os(os: abi::Os) -> metadata::loader::Os {
    use metadata::loader;

    match os {
        abi::OsWin32 => loader::OsWin32,
        abi::OsLinux => loader::OsLinux,
        abi::OsAndroid => loader::OsAndroid,
        abi::OsMacos => loader::OsMacos,
        abi::OsFreebsd => loader::OsFreebsd,
        abi::OsNaCl => loader::OsNaCl,
    }
}

#[cfg(windows)]
fn get_os_for_nacl_toolchain(_sess: &Session) -> ~str { "win".to_owned() }
#[cfg(target_os = "linux")]
fn get_os_for_nacl_toolchain(_sess: &Session) -> ~str { "linux".to_owned() }
#[cfg(target_os = "macos")]
fn get_os_for_nacl_toolchain(_sess: &Session) -> ~str { "mac".to_owned() }
#[cfg(not(windows),
      not(target_os = "linux"),
      not(target_os = "macos"))]
fn get_os_for_nacl_toolchain(sess: &Session) -> ! {
    sess.fatal("NaCl/PNaCl toolchain unsupported on this OS (update this if that's changed)");
}
