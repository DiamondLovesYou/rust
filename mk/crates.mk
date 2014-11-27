# Copyright 2014 The Rust Project Developers. See the COPYRIGHT
# file at the top-level directory of this distribution and at
# http://rust-lang.org/COPYRIGHT.
#
# Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
# http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
# <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
# option. This file may not be copied, modified, or distributed
# except according to those terms.

################################################################################
# Rust's standard distribution of crates and tools
#
# The crates outlined below are the standard distribution of libraries provided
# in a rust installation. These rules are meant to abstract over the
# dependencies (both native and rust) of crates and basically generate all the
# necessary makefile rules necessary to build everything.
#
# Here's an explanation of the variables below
#
#   TARGET_CRATES
#	This list of crates will be built for all targets, including
#	cross-compiled targets
#
#   HOST_CRATES
#	This list of crates will be compiled for only host targets. Note that
#	this set is explicitly *not* a subset of TARGET_CRATES, but rather it is
#	a disjoint set. Nothing in the TARGET_CRATES set can depend on crates in
#	the HOST_CRATES set, but the HOST_CRATES set can depend on target
#	crates.
#
#   TOOLS
#	A list of all tools which will be built as part of the compilation
#	process. It is currently assumed that most tools are built through
#	src/driver/driver.rs with a particular configuration (there's a
#	corresponding library providing the implementation)
#
#   DEPS_<crate>
#	These lists are the dependencies of the <crate> that is to be built.
#	Rust dependencies are listed bare (i.e. std) and native
#	dependencies have a "native:" prefix (i.e. native:hoedown). All deps
#	will be built before the crate itself is built.
#
#   TOOL_DEPS_<tool>/TOOL_SOURCE_<tool>
#	Similar to the DEPS variable, this is the library crate dependencies
#	list for tool as well as the source file for the specified tool
#
# You shouldn't need to modify much other than these variables. Crates are
# automatically generated for all stage/host/target combinations.
################################################################################

TARGET_CRATES := libc std flate arena term \
                 serialize getopts collections test time rand \
                 log regex graphviz core rbml alloc rustrt \
                 unicode
HOST_CRATES := syntax rustc rustc_trans rustdoc regex_macros fmt_macros \
	       rustc_llvm rustc_back rust-pnacl-trans
CRATES := $(TARGET_CRATES) $(HOST_CRATES)
TOOLS := compiletest rustdoc rustc rust-pnacl-trans

DEPS_core :=
DEPS_libc := core
DEPS_unicode := core
DEPS_alloc := core libc native:jemalloc
DEPS_rustrt := alloc core libc collections native:rustrt_native
DEPS_std := core libc rand alloc collections rustrt unicode \
	native:rust_builtin native:backtrace
DEPS_graphviz := std
DEPS_syntax := std term serialize log fmt_macros arena libc
DEPS_rustc_trans := rustc rustc_back rustc_llvm libc
DEPS_rustc := syntax flate arena serialize getopts rbml \
              time log graphviz rustc_llvm rustc_back
DEPS_rustc_llvm := native:rustllvm libc std log
DEPS_rustc_back := std syntax rustc_llvm flate log libc
DEPS_rustdoc := rustc rustc_trans native:hoedown serialize getopts \
                test time
DEPS_rust-pnacl-trans := libc getopts log rustc_llvm
DEPS_flate := std native:miniz
DEPS_arena := std
DEPS_graphviz := std
DEPS_glob := std
DEPS_serialize := std log
DEPS_rbml := std log serialize
DEPS_term := std log
DEPS_getopts := std
DEPS_collections := core alloc unicode
DEPS_num := std
DEPS_test := std getopts serialize rbml term time regex native:rust_test_helpers
DEPS_time := std serialize rustrt
DEPS_rand := core
DEPS_log := std regex
DEPS_regex := std
DEPS_regex_macros = rustc syntax std regex
DEPS_fmt_macros = std

TOOL_DEPS_compiletest := test getopts
TOOL_DEPS_rustdoc := rustdoc
TOOL_DEPS_rustc := rustc_trans
TOOL_DEPS_rust-pnacl-trans := rust-pnacl-trans
TOOL_SOURCE_compiletest := $(S)src/compiletest/compiletest.rs
TOOL_SOURCE_rustdoc := $(S)src/driver/driver.rs
TOOL_SOURCE_rustc := $(S)src/driver/driver.rs
TOOL_SOURCE_rust-pnacl-trans := $(S)src/driver/driver.rs

ONLY_RLIB_core := 1
ONLY_RLIB_libc := 1
ONLY_RLIB_alloc := 1
ONLY_RLIB_rand := 1
ONLY_RLIB_collections := 1
ONLY_RLIB_unicode := 1

################################################################################
# You should not need to edit below this line
################################################################################

DOC_CRATES := $(filter-out rustc, $(filter-out rustc_trans, $(filter-out syntax, $(CRATES))))
COMPILER_DOC_CRATES := rustc rustc_trans syntax

# This macro creates some simple definitions for each crate being built, just
# some munging of all of the parameters above.
#
# $(1) is the crate to generate variables for
define RUST_CRATE
CRATEFILE_$(1) := $$(S)src/lib$(1)/lib.rs
RSINPUTS_$(1) := $$(call rwildcard,$(S)src/lib$(1)/,*.rs)
RUST_DEPS_$(1) := $$(filter-out native:%,$$(DEPS_$(1)))
NATIVE_DEPS_$(1) := $$(patsubst native:%,%,$$(filter native:%,$$(DEPS_$(1))))
endef

$(foreach crate,$(CRATES),$(eval $(call RUST_CRATE,$(crate))))

# Similar to the macro above for crates, this macro is for tools
#
# $(1) is the crate to generate variables for
define RUST_TOOL
TOOL_INPUTS_$(1) := $$(call rwildcard,$$(dir $$(TOOL_SOURCE_$(1))),*.rs)
endef

$(foreach crate,$(TOOLS),$(eval $(call RUST_TOOL,$(crate))))
