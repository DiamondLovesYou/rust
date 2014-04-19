// Copyright 2012-2013 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use common::config;
use common::mode_compile_fail;
use common::mode_pretty;
use common::mode_run_fail;
use common::mode_run_pass;
use errors;
use header::TestProps;
use header::load_props;
use procsrv;
use util::logv;
#[cfg(target_os = "win32")]
use util;

use std::io::File;
use std::io::fs;
use std::io::net::ip::{Ipv4Addr, SocketAddr};
use std::io::net::tcp;
use std::io::process::ProcessExit;
use std::io::process;
use std::io::timer;
use std::io;
use std::os;
use std::str;
use std::task;
use std::slice;
use test::MetricMap;

pub fn run(config: config, testfile: ~str) {

    match config.target.as_slice() {

        "arm-linux-androideabi" => {
            if !config.adb_device_status {
                fail!("android device not available");
            }
        }

        _=> { }
    }

    let mut _mm = MetricMap::new();
    run_metrics(config, testfile, &mut _mm);
}

pub fn run_metrics(config: config, testfile: ~str, mm: &mut MetricMap) {
    if config.verbose {
        // We're going to be dumping a lot of info. Start on a new line.
        print!("\n\n");
    }
    let testfile = Path::new(testfile);
    debug!("running {}", testfile.display());
    let props = load_props(&testfile);
    debug!("loaded props");
    match config.mode {
      mode_compile_fail => run_cfail_test(&config, &props, &testfile),
      mode_run_fail => run_rfail_test(&config, &props, &testfile),
      mode_run_pass => run_rpass_test(&config, &props, &testfile),
      mode_pretty => run_pretty_test(&config, &props, &testfile),
      mode_debug_info => run_debuginfo_test(&config, &props, &testfile),
      mode_codegen => run_codegen_test(&config, &props, &testfile, mm)
    }
}

fn run_cfail_test(config: &config, props: &TestProps, testfile: &Path) {
    let proc_res = compile_test(config, props, testfile);

    if proc_res.status.success() {
        fatal_ProcRes(~"compile-fail test compiled successfully!", &proc_res);
    }

    check_correct_failure_status(&proc_res);

    let expected_errors = errors::load_errors(testfile);
    if !expected_errors.is_empty() {
        if !props.error_patterns.is_empty() {
            fatal(~"both error pattern and expected errors specified");
        }
        check_expected_errors(expected_errors, testfile, &proc_res);
    } else {
        check_error_patterns(props, testfile, &proc_res);
    }
}

fn run_rfail_test(config: &config, props: &TestProps, testfile: &Path) {
    let proc_res = if !config.jit {
        let proc_res = compile_test(config, props, testfile);

        if !proc_res.status.success() {
            fatal_ProcRes(~"compilation failed!", &proc_res);
        }

        exec_compiled_test(config, props, testfile)
    } else {
        jit_test(config, props, testfile)
    };

    // The value our Makefile configures valgrind to return on failure
    static VALGRIND_ERR: int = 100;
    if proc_res.status.matches_exit_status(VALGRIND_ERR) {
        fatal_ProcRes(~"run-fail test isn't valgrind-clean!", &proc_res);
    }

    check_correct_failure_status(&proc_res);
    check_error_patterns(props, testfile, &proc_res);
}

fn check_correct_failure_status(proc_res: &ProcRes) {
    // The value the rust runtime returns on failure
    static RUST_ERR: int = 101;
    if !proc_res.status.matches_exit_status(RUST_ERR) {
        fatal_ProcRes(
            format!("failure produced the wrong error: {}", proc_res.status),
            proc_res);
    }
}

fn run_rpass_test(config: &config, props: &TestProps, testfile: &Path) {
    if !config.jit {
        let mut proc_res = compile_test(config, props, testfile);

        if !proc_res.status.success() {
            fatal_ProcRes(~"compilation failed!", &proc_res);
        }

        proc_res = exec_compiled_test(config, props, testfile);

        if !proc_res.status.success() {
            fatal_ProcRes(~"test run failed!", &proc_res);
        }
    } else {
        let proc_res = jit_test(config, props, testfile);

        if !proc_res.status.success() { fatal_ProcRes(~"jit failed!", &proc_res); }
    }
}

fn run_pretty_test(config: &config, props: &TestProps, testfile: &Path) {
    if props.pp_exact.is_some() {
        logv(config, ~"testing for exact pretty-printing");
    } else { logv(config, ~"testing for converging pretty-printing"); }

    let rounds =
        match props.pp_exact { Some(_) => 1, None => 2 };

    let src = File::open(testfile).read_to_end().unwrap();
    let src = str::from_utf8(src.as_slice()).unwrap().to_owned();
    let mut srcs = vec!(src);

    let mut round = 0;
    while round < rounds {
        logv(config, format!("pretty-printing round {}", round));
        let proc_res = print_source(config,
                                    testfile,
                                    (*srcs.get(round)).clone());

        if !proc_res.status.success() {
            fatal_ProcRes(format!("pretty-printing failed in round {}", round),
                          &proc_res);
        }

        let ProcRes{ stdout, .. } = proc_res;
        srcs.push(stdout);
        round += 1;
    }

    let mut expected = match props.pp_exact {
        Some(ref file) => {
            let filepath = testfile.dir_path().join(file);
            let s = File::open(&filepath).read_to_end().unwrap();
            str::from_utf8(s.as_slice()).unwrap().to_owned()
          }
          None => { (*srcs.get(srcs.len() - 2u)).clone() }
        };
    let mut actual = (*srcs.get(srcs.len() - 1u)).clone();

    if props.pp_exact.is_some() {
        // Now we have to care about line endings
        let cr = ~"\r";
        actual = actual.replace(cr, "");
        expected = expected.replace(cr, "");
    }

    compare_source(expected, actual);

    // Finally, let's make sure it actually appears to remain valid code
    let proc_res = typecheck_source(config, props, testfile, actual);

    if !proc_res.status.success() {
        fatal_ProcRes(~"pretty-printed source does not typecheck", &proc_res);
    }

    return;

    fn print_source(config: &config, testfile: &Path, src: ~str) -> ProcRes {
        compose_and_run(config, testfile, make_pp_args(config, testfile),
                        Vec::new(), config.compile_lib_path, Some(src))
    }

    fn make_pp_args(config: &config, _testfile: &Path) -> ProcArgs {
        let args = vec!(~"-", ~"--pretty", ~"normal",
                     ~"--target=" + config.target);
        // FIXME (#9639): This needs to handle non-utf8 paths
        return ProcArgs {prog: config.rustc_path.as_str().unwrap().to_owned(), args: args};
    }

    fn compare_source(expected: &str, actual: &str) {
        if expected != actual {
            error(~"pretty-printed source does not match expected source");
            println!("\n\
expected:\n\
------------------------------------------\n\
{}\n\
------------------------------------------\n\
actual:\n\
------------------------------------------\n\
{}\n\
------------------------------------------\n\
\n",
                     expected, actual);
            fail!();
        }
    }

    fn typecheck_source(config: &config, props: &TestProps,
                        testfile: &Path, src: ~str) -> ProcRes {
        let args = make_typecheck_args(config, props, testfile);
        compose_and_run_compiler(config, props, testfile, args, Some(src))
    }

    fn make_typecheck_args(config: &config, props: &TestProps, testfile: &Path) -> ProcArgs {
        let aux_dir = aux_output_dir_name(config, testfile);
        let target = if props.force_host {
            config.host.as_slice()
        } else {
            config.target.as_slice()
        };
        // FIXME (#9639): This needs to handle non-utf8 paths
        let mut args = vec!(~"-",
                         ~"--no-trans", ~"--crate-type=lib",
                         ~"--target=" + target,
                         ~"-L", config.build_base.as_str().unwrap().to_owned(),
                         ~"-L",
                         aux_dir.as_str().unwrap().to_owned());
        args.push_all_move(split_maybe_args(&config.target_rustcflags));
        args.push_all_move(split_maybe_args(&props.compile_flags));
        // FIXME (#9639): This needs to handle non-utf8 paths
        return ProcArgs {prog: config.rustc_path.as_str().unwrap().to_owned(), args: args};
    }
}

fn run_debuginfo_test(config: &config, props: &TestProps, testfile: &Path) {
    let mut config = config {
        target_rustcflags: cleanup_debug_info_options(&config.target_rustcflags),
        host_rustcflags: cleanup_debug_info_options(&config.host_rustcflags),
        .. config.clone()
    };

    let config = &mut config;
    let check_lines = &props.check_lines;
    let mut cmds = props.debugger_cmds.connect("\n");

    // compile test file (it shoud have 'compile-flags:-g' in the header)
    let mut proc_res = compile_test(config, props, testfile);
    if !proc_res.status.success() {
        fatal_ProcRes(~"compilation failed!", &proc_res);
    }

    let exe_file = make_exe_name(config, testfile);

    let mut proc_args;
    match config.target.as_slice() {
        "arm-linux-androideabi" => {

            cmds = cmds.replace("run","continue");

            // write debugger script
            let script_str = [~"set charset UTF-8",
                              format!("file {}",exe_file.as_str().unwrap().to_owned()),
                              ~"target remote :5039",
                              cmds,
                              ~"quit"].connect("\n");
            debug!("script_str = {}", script_str);
            dump_output_file(config, testfile, script_str, "debugger.script");


            procsrv::run("", config.adb_path,
                         [~"push", exe_file.as_str().unwrap().to_owned(),
                          config.adb_test_dir.clone()],
                         vec!((~"",~"")), Some(~""))
                .expect(format!("failed to exec `{}`", config.adb_path));

            procsrv::run("", config.adb_path,
                         [~"forward", ~"tcp:5039", ~"tcp:5039"],
                         vec!((~"",~"")), Some(~""))
                .expect(format!("failed to exec `{}`", config.adb_path));

            let adb_arg = format!("export LD_LIBRARY_PATH={}; gdbserver :5039 {}/{}",
                                  config.adb_test_dir.clone(), config.adb_test_dir.clone(),
                                  str::from_utf8(exe_file.filename().unwrap()).unwrap());

            let mut process = procsrv::run_background("", config.adb_path,
                                                      [~"shell",adb_arg.clone()],
                                                      vec!((~"",~"")), Some(~""))
                .expect(format!("failed to exec `{}`", config.adb_path));
            loop {
                //waiting 1 second for gdbserver start
                timer::sleep(1000);
                let result = task::try(proc() {
                    tcp::TcpStream::connect(SocketAddr {
                        ip: Ipv4Addr(127, 0, 0, 1),
                        port: 5039,
                    }).unwrap();
                });
                if result.is_err() {
                    continue;
                }
                break;
            }

            let args = split_maybe_args(&config.target_rustcflags);
            let mut tool_path:~str = ~"";
            for arg in args.iter() {
                if arg.contains("android-cross-path=") {
                    tool_path = arg.replace("android-cross-path=","");
                    break;
                }
            }

            if tool_path.is_empty() {
                fatal(~"cannot found android cross path");
            }

            let debugger_script = make_out_name(config, testfile, "debugger.script");
            // FIXME (#9639): This needs to handle non-utf8 paths
            let debugger_opts = vec!(~"-quiet", ~"-batch", ~"-nx",
                                  "-command=" + debugger_script.as_str().unwrap().to_owned());

            let gdb_path = tool_path.append("/bin/arm-linux-androideabi-gdb");
            let procsrv::Result{ out, err, status }=
                procsrv::run("",
                             gdb_path,
                             debugger_opts.as_slice(),
                             vec!((~"",~"")),
                             None)
                .expect(format!("failed to exec `{}`", gdb_path));
            let cmdline = {
                let cmdline = make_cmdline("",
                                           "arm-linux-androideabi-gdb",
                                           debugger_opts.as_slice());
                logv(config, format!("executing {}", cmdline));
                cmdline
            };

            proc_res = ProcRes {status: status,
                               stdout: out,
                               stderr: err,
                               cmdline: cmdline};
            process.signal_kill().unwrap();
        }

        "le32-unknown-nacl" => {
        }

        _=> {
            // write debugger script
            let script_str = [~"set charset UTF-8",
                cmds,
                ~"quit\n"].connect("\n");
            debug!("script_str = {}", script_str);
            dump_output_file(config, testfile, script_str, "debugger.script");

            // run debugger script with gdb
            #[cfg(windows)]
            fn debugger() -> ~str { ~"gdb.exe" }
            #[cfg(unix)]
            fn debugger() -> ~str { ~"gdb" }

            let debugger_script = make_out_name(config, testfile, "debugger.script");

            // FIXME (#9639): This needs to handle non-utf8 paths
            let debugger_opts = vec!(~"-quiet", ~"-batch", ~"-nx",
                "-command=" + debugger_script.as_str().unwrap().to_owned(),
                exe_file.as_str().unwrap().to_owned());
            proc_args = ProcArgs {prog: debugger(), args: debugger_opts};
            proc_res = compose_and_run(config, testfile, proc_args, Vec::new(), "", None);
        }
    }

    if !proc_res.status.success() {
        fatal(~"gdb failed to execute");
    }
    let num_check_lines = check_lines.len();
    if num_check_lines > 0 {
        // Allow check lines to leave parts unspecified (e.g., uninitialized
        // bits in the wrong case of an enum) with the notation "[...]".
        let check_fragments: Vec<Vec<~str>> =
            check_lines.iter().map(|s| {
                s.split_str("[...]").map(|x| x.to_str()).collect()
            }).collect();
        // check if each line in props.check_lines appears in the
        // output (in order)
        let mut i = 0u;
        for line in proc_res.stdout.lines() {
            let mut rest = line.trim();
            let mut first = true;
            let mut failed = false;
            for frag in check_fragments.get(i).iter() {
                let found = if first {
                    if rest.starts_with(*frag) { Some(0) } else { None }
                } else {
                    rest.find_str(*frag)
                };
                match found {
                    None => {
                        failed = true;
                        break;
                    }
                    Some(i) => {
                        rest = rest.slice_from(i + frag.len());
                    }
                }
                first = false;
            }
            if !failed && rest.len() == 0 {
                i += 1u;
            }
            if i == num_check_lines {
                // all lines checked
                break;
            }
        }
        if i != num_check_lines {
            fatal_ProcRes(format!("line not found in debugger output: {}",
                                  *check_lines.get(i)), &proc_res);
        }
    }

    fn cleanup_debug_info_options(options: &Option<~str>) -> Option<~str> {
        if options.is_none() {
            return None;
        }

        // Remove options that are either unwanted (-O) or may lead to duplicates due to RUSTFLAGS.
        let options_to_remove = [~"-O", ~"-g", ~"--debuginfo"];
        let new_options = split_maybe_args(options).move_iter()
                                                   .filter(|x| !options_to_remove.contains(x))
                                                   .collect::<~[~str]>()
                                                   .connect(" ");
        Some(new_options)
    }
}

fn check_error_patterns(props: &TestProps,
                        testfile: &Path,
                        proc_res: &ProcRes) {
    if props.error_patterns.is_empty() {
        fatal(~"no error pattern specified in " + testfile.display().as_maybe_owned().as_slice());
    }

    if proc_res.status.success() {
        fatal(~"process did not return an error status");
    }

    let mut next_err_idx = 0u;
    let mut next_err_pat = props.error_patterns.get(next_err_idx);
    let mut done = false;
    let output_to_check = if props.check_stdout {
        proc_res.stdout + proc_res.stderr
    } else {
        proc_res.stderr.clone()
    };
    for line in output_to_check.lines() {
        if line.contains(*next_err_pat) {
            debug!("found error pattern {}", *next_err_pat);
            next_err_idx += 1u;
            if next_err_idx == props.error_patterns.len() {
                debug!("found all error patterns");
                done = true;
                break;
            }
            next_err_pat = props.error_patterns.get(next_err_idx);
        }
    }
    if done { return; }

    let missing_patterns =
        props.error_patterns.slice(next_err_idx, props.error_patterns.len());
    if missing_patterns.len() == 1u {
        fatal_ProcRes(format!("error pattern '{}' not found!",
                              missing_patterns[0]), proc_res);
    } else {
        for pattern in missing_patterns.iter() {
            error(format!("error pattern '{}' not found!", *pattern));
        }
        fatal_ProcRes(~"multiple error patterns not found", proc_res);
    }
}

fn check_expected_errors(expected_errors: Vec<errors::ExpectedError> ,
                         testfile: &Path,
                         proc_res: &ProcRes) {

    // true if we found the error in question
    let mut found_flags = slice::from_elem(
        expected_errors.len(), false);

    if proc_res.status.success() {
        fatal(~"process did not return an error status");
    }

    let prefixes = expected_errors.iter().map(|ee| {
        format!("{}:{}:", testfile.display(), ee.line)
    }).collect::<Vec<~str> >();

    #[cfg(target_os = "win32")]
    fn to_lower( s : &str ) -> ~str {
        let i = s.chars();
        let c : Vec<char> = i.map( |c| {
            if c.is_ascii() {
                c.to_ascii().to_lower().to_char()
            } else {
                c
            }
        } ).collect();
        str::from_chars(c.as_slice())
    }

    #[cfg(target_os = "win32")]
    fn prefix_matches( line : &str, prefix : &str ) -> bool {
        to_lower(line).starts_with( to_lower(prefix) )
    }

    #[cfg(target_os = "linux")]
    #[cfg(target_os = "macos")]
    #[cfg(target_os = "freebsd")]
    fn prefix_matches( line : &str, prefix : &str ) -> bool {
        line.starts_with( prefix )
    }

    // Scan and extract our error/warning messages,
    // which look like:
    //    filename:line1:col1: line2:col2: *error:* msg
    //    filename:line1:col1: line2:col2: *warning:* msg
    // where line1:col1: is the starting point, line2:col2:
    // is the ending point, and * represents ANSI color codes.
    for line in proc_res.stderr.lines() {
        let mut was_expected = false;
        for (i, ee) in expected_errors.iter().enumerate() {
            if !found_flags[i] {
                debug!("prefix={} ee.kind={} ee.msg={} line={}",
                       *prefixes.get(i), ee.kind, ee.msg, line);
                if prefix_matches(line, *prefixes.get(i)) &&
                    line.contains(ee.kind) &&
                    line.contains(ee.msg) {
                    found_flags[i] = true;
                    was_expected = true;
                    break;
                }
            }
        }

        // ignore this msg which gets printed at the end
        if line.contains("aborting due to") {
            was_expected = true;
        }

        if !was_expected && is_compiler_error_or_warning(line) {
            fatal_ProcRes(format!("unexpected compiler error or warning: '{}'",
                               line),
                          proc_res);
        }
    }

    for (i, &flag) in found_flags.iter().enumerate() {
        if !flag {
            let ee = expected_errors.get(i);
            fatal_ProcRes(format!("expected {} on line {} not found: {}",
                               ee.kind, ee.line, ee.msg), proc_res);
        }
    }
}

fn is_compiler_error_or_warning(line: &str) -> bool {
    let mut i = 0u;
    return
        scan_until_char(line, ':', &mut i) &&
        scan_char(line, ':', &mut i) &&
        scan_integer(line, &mut i) &&
        scan_char(line, ':', &mut i) &&
        scan_integer(line, &mut i) &&
        scan_char(line, ':', &mut i) &&
        scan_char(line, ' ', &mut i) &&
        scan_integer(line, &mut i) &&
        scan_char(line, ':', &mut i) &&
        scan_integer(line, &mut i) &&
        scan_char(line, ' ', &mut i) &&
        (scan_string(line, "error", &mut i) ||
         scan_string(line, "warning", &mut i));
}

fn scan_until_char(haystack: &str, needle: char, idx: &mut uint) -> bool {
    if *idx >= haystack.len() {
        return false;
    }
    let opt = haystack.slice_from(*idx).find(needle);
    if opt.is_none() {
        return false;
    }
    *idx = opt.unwrap();
    return true;
}

fn scan_char(haystack: &str, needle: char, idx: &mut uint) -> bool {
    if *idx >= haystack.len() {
        return false;
    }
    let range = haystack.char_range_at(*idx);
    if range.ch != needle {
        return false;
    }
    *idx = range.next;
    return true;
}

fn scan_integer(haystack: &str, idx: &mut uint) -> bool {
    let mut i = *idx;
    while i < haystack.len() {
        let range = haystack.char_range_at(i);
        if range.ch < '0' || '9' < range.ch {
            break;
        }
        i = range.next;
    }
    if i == *idx {
        return false;
    }
    *idx = i;
    return true;
}

fn scan_string(haystack: &str, needle: &str, idx: &mut uint) -> bool {
    let mut haystack_i = *idx;
    let mut needle_i = 0u;
    while needle_i < needle.len() {
        if haystack_i >= haystack.len() {
            return false;
        }
        let range = haystack.char_range_at(haystack_i);
        haystack_i = range.next;
        if !scan_char(needle, range.ch, &mut needle_i) {
            return false;
        }
    }
    *idx = haystack_i;
    return true;
}

struct ProcArgs {prog: ~str, args: Vec<~str> }

struct ProcRes {status: ProcessExit, stdout: ~str, stderr: ~str, cmdline: ~str}

fn compile_test(config: &config, props: &TestProps,
                testfile: &Path) -> ProcRes {
    compile_test_(config, props, testfile, [])
}

fn jit_test(config: &config, props: &TestProps, testfile: &Path) -> ProcRes {
    compile_test_(config, props, testfile, [~"--jit"])
}

fn compile_test_(config: &config, props: &TestProps,
                 testfile: &Path, extra_args: &[~str]) -> ProcRes {
    let aux_dir = aux_output_dir_name(config, testfile);
    // FIXME (#9639): This needs to handle non-utf8 paths
    let link_args = vec!(~"-L",
                         aux_dir.as_str().unwrap().to_owned());
    let link_args = if config.targeting_pnacl() {
        // As a result of the way we link in rustc, targeting pnacl requires use of LTO.
        link_args.append(vec!(~"-Z", ~"lto").as_slice())
    } else {
        link_args
    };
    let args = make_compile_args(config,
                                 props,
                                 link_args.append(extra_args),
                                 |a, b| ThisFile(make_exe_name(a, b)), testfile);
    compose_and_run_compiler(config, props, testfile, args, None)
}

fn exec_compiled_test(config: &config, props: &TestProps,
                      testfile: &Path) -> ProcRes {

    let env = props.exec_env.clone();

    match config.target.as_slice() {

        "arm-linux-androideabi" => {
            _arm_exec_compiled_test(config, props, testfile, env)
        }

        "le32-unknown-nacl" => {
            pnacl_exec_compiled_test(config, props, testfile, env)
        }

        _=> {
            compose_and_run(config, testfile,
                            make_run_args(config, props, testfile),
                            env,
                            config.run_lib_path, None)
        }
    }
}

fn compose_and_run_compiler(
    config: &config,
    props: &TestProps,
    testfile: &Path,
    args: ProcArgs,
    input: Option<~str>) -> ProcRes {

    if !props.aux_builds.is_empty() {
        ensure_dir(&aux_output_dir_name(config, testfile));
    }

    let aux_dir = aux_output_dir_name(config, testfile);
    // FIXME (#9639): This needs to handle non-utf8 paths
    let extra_link_args = vec!(~"-L", aux_dir.as_str().unwrap().to_owned());

    for rel_ab in props.aux_builds.iter() {
        let abs_ab = config.aux_base.join(rel_ab.as_slice());
        let aux_props = load_props(&abs_ab);
        let crate_type = if aux_props.no_prefer_dynamic && !config.targeting_nacl() {
            Vec::new()
        } else if config.targeting_nacl() {
            vec!(~"--crate-type=rlib")
        } else {
            vec!(~"--crate-type=dylib")
        };
        let aux_args =
            make_compile_args(config,
                              &aux_props,
                              crate_type.append(extra_link_args.as_slice()),
                              |a,b| {
                                  let f = make_lib_name(a, b, testfile);
                                  ThisDirectory(f.dir_path())
                              }, &abs_ab);
        let auxres = compose_and_run(config, &abs_ab, aux_args, Vec::new(),
                                     config.compile_lib_path, None);
        if !auxres.status.success() {
            fatal_ProcRes(
                format!("auxiliary build of {} failed to compile: ",
                     abs_ab.display()),
                &auxres);
        }

        match config.target.as_slice() {

            "arm-linux-androideabi" => {
                _arm_push_aux_shared_library(config, testfile);
            }

            _=> { }
        }
    }

    compose_and_run(config, testfile, args, Vec::new(),
                    config.compile_lib_path, input)
}

fn ensure_dir(path: &Path) {
    if path.is_dir() { return; }
    fs::mkdir(path, io::UserRWX).unwrap();
}

fn compose_and_run(config: &config, testfile: &Path,
                   ProcArgs{ args, prog }: ProcArgs,
                   procenv: Vec<(~str, ~str)> ,
                   lib_path: &str,
                   input: Option<~str>) -> ProcRes {
    return program_output(config, testfile, lib_path,
                          prog, args, procenv, input);
}

enum TargetLocation {
    ThisFile(Path),
    ThisDirectory(Path),
}

fn make_compile_args(config: &config,
                     props: &TestProps,
                     extras: Vec<~str> ,
                     xform: |&config, &Path| -> TargetLocation,
                     testfile: &Path)
                     -> ProcArgs {
    let xform_file = xform(config, testfile);
    let target = if props.force_host {
        config.host.as_slice()
    } else {
        config.target.as_slice()
    };
    // FIXME (#9639): This needs to handle non-utf8 paths
    let mut args = vec!(testfile.as_str().unwrap().to_owned(),
                     ~"-L", config.build_base.as_str().unwrap().to_owned(),
                     ~"--target=" + target);
    args.push_all(extras.as_slice());
    if !props.no_prefer_dynamic && !config.targeting_nacl() {
        args.push(~"-C");
        args.push(~"prefer-dynamic");
    }
    let path = match xform_file {
        ThisFile(path) => { args.push(~"-o"); path }
        ThisDirectory(path) => { args.push(~"--out-dir"); path }
    };
    args.push(os::getcwd().join(path).as_str().unwrap().to_owned());
    if props.force_host {
        args.push_all_move(split_maybe_args(&config.host_rustcflags));
    } else {
        args.push_all_move(split_maybe_args(&config.target_rustcflags));
    }
    args.push_all_move(split_maybe_args(&props.compile_flags));
    return ProcArgs {prog: config.rustc_path.as_str().unwrap().to_owned(), args: args};
}

fn make_lib_name(config: &config, auxfile: &Path, testfile: &Path) -> Path {
    // what we return here is not particularly important, as it
    // happens; rustc ignores everything except for the directory.
    let auxname = output_testname(auxfile);
    aux_output_dir_name(config, testfile).join(&auxname)
}

fn make_exe_name(config: &config, testfile: &Path) -> Path {
    let mut f = output_base_name(config, testfile);
    if !os::consts::EXE_SUFFIX.is_empty() {
        match f.filename().map(|s| s + os::consts::EXE_SUFFIX.as_bytes()) {
            Some(v) => f.set_filename(v),
            None => ()
        }
    }
    f
}

fn make_run_args(config: &config, _props: &TestProps, testfile: &Path) ->
   ProcArgs {
    // If we've got another tool to run under (valgrind),
    // then split apart its command
    let mut args = split_maybe_args(&config.runtool);
    let exe_file = make_exe_name(config, testfile);
    // FIXME (#9639): This needs to handle non-utf8 paths
    args.push(exe_file.as_str().unwrap().to_owned());
    let prog = args.shift().unwrap();
    return ProcArgs {prog: prog, args: args};
}

fn split_maybe_args(argstr: &Option<~str>) -> Vec<~str> {
    match *argstr {
        Some(ref s) => {
            s.split(' ')
                .filter_map(|s| if s.is_whitespace() {None} else {Some(s.to_owned())})
                .collect()
        }
        None => Vec::new()
    }
}

fn program_output(config: &config, testfile: &Path, lib_path: &str, prog: ~str,
                  args: Vec<~str> , env: Vec<(~str, ~str)> ,
                  input: Option<~str>) -> ProcRes {
    let cmdline =
        {
            let cmdline = make_cmdline(lib_path, prog, args.as_slice());
            logv(config, format!("executing {}", cmdline));
            cmdline
        };
    let procsrv::Result{ out, err, status } =
            procsrv::run(lib_path, prog, args.as_slice(), env, input)
            .expect(format!("failed to exec `{}`", prog));
    dump_output(config, testfile, out, err);
    return ProcRes {status: status,
         stdout: out,
         stderr: err,
         cmdline: cmdline};
}

// Linux and mac don't require adjusting the library search path
#[cfg(target_os = "linux")]
#[cfg(target_os = "macos")]
#[cfg(target_os = "freebsd")]
fn make_cmdline(_libpath: &str, prog: &str, args: &[~str]) -> ~str {
    format!("{} {}", prog, args.connect(" "))
}

#[cfg(target_os = "win32")]
fn make_cmdline(libpath: &str, prog: &str, args: &[~str]) -> ~str {
    format!("{} {} {}", lib_path_cmd_prefix(libpath), prog,
         args.connect(" "))
}

// Build the LD_LIBRARY_PATH variable as it would be seen on the command line
// for diagnostic purposes
#[cfg(target_os = "win32")]
fn lib_path_cmd_prefix(path: &str) -> ~str {
    format!("{}=\"{}\"", util::lib_path_env_var(), util::make_new_path(path))
}

fn dump_output(config: &config, testfile: &Path, out: &str, err: &str) {
    dump_output_file(config, testfile, out, "out");
    dump_output_file(config, testfile, err, "err");
    maybe_dump_to_stdout(config, out, err);
}

fn dump_output_file(config: &config, testfile: &Path,
                    out: &str, extension: &str) {
    let outfile = make_out_name(config, testfile, extension);
    File::create(&outfile).write(out.as_bytes()).unwrap();
}

fn make_out_name(config: &config, testfile: &Path, extension: &str) -> Path {
    output_base_name(config, testfile).with_extension(extension)
}

fn aux_output_dir_name(config: &config, testfile: &Path) -> Path {
    let mut f = output_base_name(config, testfile);
    match f.filename().map(|s| s + bytes!(".libaux")) {
        Some(v) => f.set_filename(v),
        None => ()
    }
    f
}

fn output_testname(testfile: &Path) -> Path {
    Path::new(testfile.filestem().unwrap())
}

fn output_base_name(config: &config, testfile: &Path) -> Path {
    config.build_base
        .join(&output_testname(testfile))
        .with_extension(config.stage_id.as_slice())
}

fn maybe_dump_to_stdout(config: &config, out: &str, err: &str) {
    if config.verbose {
        println!("------{}------------------------------", "stdout");
        println!("{}", out);
        println!("------{}------------------------------", "stderr");
        println!("{}", err);
        println!("------------------------------------------");
    }
}

fn error(err: ~str) { println!("\nerror: {}", err); }

fn fatal(err: ~str) -> ! { error(err); fail!(); }

fn fatal_ProcRes(err: ~str, proc_res: &ProcRes) -> ! {
    print!("\n\
error: {}\n\
command: {}\n\
stdout:\n\
------------------------------------------\n\
{}\n\
------------------------------------------\n\
stderr:\n\
------------------------------------------\n\
{}\n\
------------------------------------------\n\
\n",
             err, proc_res.cmdline, proc_res.stdout, proc_res.stderr);
    fail!();
}

fn _arm_exec_compiled_test(config: &config, props: &TestProps,
                      testfile: &Path, env: Vec<(~str, ~str)> ) -> ProcRes {

    let args = make_run_args(config, props, testfile);
    let cmdline = make_cmdline("", args.prog, args.args.as_slice());

    // get bare program string
    let mut tvec: Vec<~str> = args.prog.split('/').map(|ts| ts.to_owned()).collect();
    let prog_short = tvec.pop().unwrap();

    // copy to target
    let copy_result = procsrv::run("", config.adb_path,
        [~"push", args.prog.clone(), config.adb_test_dir.clone()],
        vec!((~"",~"")), Some(~""))
        .expect(format!("failed to exec `{}`", config.adb_path));

    if config.verbose {
        println!("push ({}) {} {} {}",
            config.target, args.prog,
            copy_result.out, copy_result.err);
    }

    logv(config, format!("executing ({}) {}", config.target, cmdline));

    let mut runargs = Vec::new();

    // run test via adb_run_wrapper
    runargs.push(~"shell");
    for (key, val) in env.move_iter() {
        runargs.push(format!("{}={}", key, val));
    }
    runargs.push(format!("{}/adb_run_wrapper.sh", config.adb_test_dir));
    runargs.push(format!("{}", config.adb_test_dir));
    runargs.push(format!("{}", prog_short));

    for tv in args.args.iter() {
        runargs.push(tv.to_owned());
    }
    procsrv::run("",
                 config.adb_path,
                 runargs.as_slice(),
                 vec!((~"",~"")), Some(~""))
        .expect(format!("failed to exec `{}`", config.adb_path));

    // get exitcode of result
    runargs = Vec::new();
    runargs.push(~"shell");
    runargs.push(~"cat");
    runargs.push(format!("{}/{}.exitcode", config.adb_test_dir, prog_short));

    let procsrv::Result{ out: exitcode_out, err: _, status: _ } =
        procsrv::run("", config.adb_path, runargs.as_slice(), vec!((~"",~"")),
                     Some(~""))
        .expect(format!("failed to exec `{}`", config.adb_path));

    let mut exitcode : int = 0;
    for c in exitcode_out.chars() {
        if !c.is_digit() { break; }
        exitcode = exitcode * 10 + match c {
            '0' .. '9' => c as int - ('0' as int),
            _ => 101,
        }
    }

    // get stdout of result
    runargs = Vec::new();
    runargs.push(~"shell");
    runargs.push(~"cat");
    runargs.push(format!("{}/{}.stdout", config.adb_test_dir, prog_short));

    let procsrv::Result{ out: stdout_out, err: _, status: _ } =
        procsrv::run("",
                     config.adb_path,
                     runargs.as_slice(),
                     vec!((~"",~"")), Some(~""))
        .expect(format!("failed to exec `{}`", config.adb_path));

    // get stderr of result
    runargs = Vec::new();
    runargs.push(~"shell");
    runargs.push(~"cat");
    runargs.push(format!("{}/{}.stderr", config.adb_test_dir, prog_short));

    let procsrv::Result{ out: stderr_out, err: _, status: _ } =
        procsrv::run("",
                     config.adb_path,
                     runargs.as_slice(),
                     vec!((~"",~"")), Some(~""))
        .expect(format!("failed to exec `{}`", config.adb_path));

    dump_output(config, testfile, stdout_out, stderr_out);

    ProcRes {
        status: process::ExitStatus(exitcode),
        stdout: stdout_out,
        stderr: stderr_out,
        cmdline: cmdline
    }
}

fn _arm_push_aux_shared_library(config: &config, testfile: &Path) {
    let tdir = aux_output_dir_name(config, testfile);

    let dirs = fs::readdir(&tdir).unwrap();
    for file in dirs.iter() {
        if file.extension_str() == Some("so") {
            // FIXME (#9639): This needs to handle non-utf8 paths
            let copy_result = procsrv::run("", config.adb_path,
                [~"push", file.as_str().unwrap().to_owned(), config.adb_test_dir.clone()],
                vec!((~"",~"")), Some(~""))
                .expect(format!("failed to exec `{}`", config.adb_path));

            if config.verbose {
                println!("push ({}) {} {} {}",
                    config.target, file.display(),
                    copy_result.out, copy_result.err);
            }
        }
    }
}

fn pnacl_exec_compiled_test(config: &config, props: &TestProps,
                            testfile: &Path, env: Vec<(~str, ~str)> ) -> ProcRes {
    use std::os::consts::ARCH;
    use std::os::make_absolute;
    use std::io::process::{ExitStatus};
    let cross_path = config.nacl_cross_path
        .clone()
        .expect("need the NaCl SDK path!");
    let pnacl_translate = cross_path.join_many([~"toolchain",
                                                toolchain_prefix() + "_pnacl",
                                                ~"bin",
                                                ~"pnacl-translate"]);

    let pexe_path = make_absolute(&output_base_name(config, testfile));
    let nexe_path = 
        // add an extension, don't replace it:
        Path::new(pexe_path.display().to_str() + ".nexe");
    
    let pnacl_trans_args = vec!(~"-O0",
                                ~"-arch",
                                ARCH.to_str(),
                                ~"-o",
                                nexe_path.display().to_str(),
                                pexe_path.display().to_str(),
                                ~"--pnacl-allow-zerocost-eh",
                                ~"--allow-llvm-bitcode-input",
                                ~"--pnacl-driver-verbose");
    let procsrv::Result { out: stdout, err: stderr, status: status } =
        procsrv::run("",
                     pnacl_translate.display().to_str(),
                     pnacl_trans_args.as_slice(),
                     env.clone(),
                     None)
        .expect(format!("failed to exec `{}`", pnacl_translate.display()));
    match status {
        ExitStatus(0) => (),
        _ => {
            return ProcRes {
                status: status,
                stdout: stdout,
                stderr: stderr,
                cmdline: format!("{} {}",
                                 pnacl_translate.display(),
                                 pnacl_trans_args.connect(" ")),
            };
        }
    }

    let sel_ldr = cross_path.join_many(["tools",
                                        "sel_ldr.py"]);
    let sel_ldr_args = vec!(~"--debug-libs", ~"-v", ~"--", nexe_path.display().to_str());
    let ProcArgs {
        args: run_args,
        ..
    } = make_run_args(config, props, testfile);
    let sel_ldr_args = sel_ldr_args.append(run_args.as_slice());
    let procsrv::Result{ out: stdout, err: stderr, status: status } =
        procsrv::run("",
                     sel_ldr.display().to_str(),
                     sel_ldr_args.as_slice(),
                     env,
                     None).unwrap();
    return ProcRes {
        status: status,
        stdout: stdout,
        stderr: stderr,
        cmdline: make_cmdline("",
                              sel_ldr.display().to_str(),
                              sel_ldr_args.as_slice()),
    };

    #[cfg(target_os = "linux")]
    fn toolchain_prefix() -> &'static str {
        "linux"
    }
}

// codegen tests (vs. clang)

fn make_o_name(config: &config, testfile: &Path) -> Path {
    output_base_name(config, testfile).with_extension("o")
}

fn append_suffix_to_stem(p: &Path, suffix: &str) -> Path {
    if suffix.len() == 0 {
        (*p).clone()
    } else {
        let stem = p.filestem().unwrap();
        p.with_filename(stem + bytes!("-") + suffix.as_bytes())
    }
}

fn compile_test_and_save_bitcode(config: &config, props: &TestProps,
                                 testfile: &Path) -> ProcRes {
    let aux_dir = aux_output_dir_name(config, testfile);
    // FIXME (#9639): This needs to handle non-utf8 paths
    let link_args = vec!(~"-L", aux_dir.as_str().unwrap().to_owned());
    let llvm_args = vec!(~"--emit=obj", ~"--crate-type=lib", ~"-C", ~"save-temps");
    let args = make_compile_args(config,
                                 props,
                                 link_args.append(llvm_args.as_slice()),
                                 |a, b| ThisFile(make_o_name(a, b)), testfile);
    compose_and_run_compiler(config, props, testfile, args, None)
}

fn compile_cc_with_clang_and_save_bitcode(config: &config, _props: &TestProps,
                                          testfile: &Path) -> ProcRes {
    let bitcodefile = output_base_name(config, testfile).with_extension("bc");
    let bitcodefile = append_suffix_to_stem(&bitcodefile, "clang");
    let testcc = testfile.with_extension("cc");
    let proc_args = ProcArgs {
        // FIXME (#9639): This needs to handle non-utf8 paths
        prog: config.clang_path.get_ref().as_str().unwrap().to_owned(),
        args: vec!(~"-c",
                ~"-emit-llvm",
                ~"-o", bitcodefile.as_str().unwrap().to_owned(),
                testcc.as_str().unwrap().to_owned() )
    };
    compose_and_run(config, testfile, proc_args, Vec::new(), "", None)
}

fn extract_function_from_bitcode(config: &config, _props: &TestProps,
                                 fname: &str, testfile: &Path,
                                 suffix: &str) -> ProcRes {
    let bitcodefile = output_base_name(config, testfile).with_extension("bc");
    let bitcodefile = append_suffix_to_stem(&bitcodefile, suffix);
    let extracted_bc = append_suffix_to_stem(&bitcodefile, "extract");
    let prog = config.llvm_bin_path.get_ref().join("llvm-extract");
    let proc_args = ProcArgs {
        // FIXME (#9639): This needs to handle non-utf8 paths
        prog: prog.as_str().unwrap().to_owned(),
        args: vec!("-func=" + fname,
                "-o=" + extracted_bc.as_str().unwrap(),
                bitcodefile.as_str().unwrap().to_owned() )
    };
    compose_and_run(config, testfile, proc_args, Vec::new(), "", None)
}

fn disassemble_extract(config: &config, _props: &TestProps,
                       testfile: &Path, suffix: &str) -> ProcRes {
    let bitcodefile = output_base_name(config, testfile).with_extension("bc");
    let bitcodefile = append_suffix_to_stem(&bitcodefile, suffix);
    let extracted_bc = append_suffix_to_stem(&bitcodefile, "extract");
    let extracted_ll = extracted_bc.with_extension("ll");
    let prog = config.llvm_bin_path.get_ref().join("llvm-dis");
    let proc_args = ProcArgs {
        // FIXME (#9639): This needs to handle non-utf8 paths
        prog: prog.as_str().unwrap().to_owned(),
        args: vec!("-o=" + extracted_ll.as_str().unwrap(),
                extracted_bc.as_str().unwrap().to_owned() )
    };
    compose_and_run(config, testfile, proc_args, Vec::new(), "", None)
}


fn count_extracted_lines(p: &Path) -> uint {
    let x = File::open(&p.with_extension("ll")).read_to_end().unwrap();
    let x = str::from_utf8(x.as_slice()).unwrap();
    x.lines().len()
}


fn run_codegen_test(config: &config, props: &TestProps,
                    testfile: &Path, mm: &mut MetricMap) {

    if config.llvm_bin_path.is_none() {
        fatal(~"missing --llvm-bin-path");
    }

    if config.clang_path.is_none() {
        fatal(~"missing --clang-path");
    }

    let mut proc_res = compile_test_and_save_bitcode(config, props, testfile);
    if !proc_res.status.success() {
        fatal_ProcRes(~"compilation failed!", &proc_res);
    }

    proc_res = extract_function_from_bitcode(config, props, "test", testfile, "");
    if !proc_res.status.success() {
        fatal_ProcRes(~"extracting 'test' function failed", &proc_res);
    }

    proc_res = disassemble_extract(config, props, testfile, "");
    if !proc_res.status.success() {
        fatal_ProcRes(~"disassembling extract failed", &proc_res);
    }


    let mut proc_res = compile_cc_with_clang_and_save_bitcode(config, props, testfile);
    if !proc_res.status.success() {
        fatal_ProcRes(~"compilation failed!", &proc_res);
    }

    proc_res = extract_function_from_bitcode(config, props, "test", testfile, "clang");
    if !proc_res.status.success() {
        fatal_ProcRes(~"extracting 'test' function failed", &proc_res);
    }

    proc_res = disassemble_extract(config, props, testfile, "clang");
    if !proc_res.status.success() {
        fatal_ProcRes(~"disassembling extract failed", &proc_res);
    }

    let base = output_base_name(config, testfile);
    let base_extract = append_suffix_to_stem(&base, "extract");

    let base_clang = append_suffix_to_stem(&base, "clang");
    let base_clang_extract = append_suffix_to_stem(&base_clang, "extract");

    let base_lines = count_extracted_lines(&base_extract);
    let clang_lines = count_extracted_lines(&base_clang_extract);

    mm.insert_metric("clang-codegen-ratio",
                     (base_lines as f64) / (clang_lines as f64),
                     0.001);
}
