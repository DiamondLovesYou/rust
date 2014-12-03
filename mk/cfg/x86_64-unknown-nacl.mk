
NACL_TC_PATH := $(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_x86_newlib

# x86_64-unknown-nacl (non-portable)
CC_x86_64-unknown-nacl=$(NACL_TC_PATH)/bin/x86_64-nacl-gcc
CXX_x86_64-unknown-nacl=$(NACL_TC_PATH)/bin/x86_64-nacl-g++
CPP_x86_64-unknown-nacl=$(CXX_x86_64-unknown-nacl) -E
AR_x86_64-unknown-nacl=$(NACL_TC_PATH)/bin/x86_64-nacl-ar
CFG_LIB_NAME_x86_64-unknown-nacl=lib$(1).so
CFG_STATIC_LIB_NAME_x86_64-unknown-nacl=lib$(1).a
CFG_LIB_GLOB_x86_64-unknown-nacl=lib$(1)-*.so
CFG_LIB_DSYM_GLOB_x86_64-unknown-nacl=lib$(1)-*.dylib.dSYM
CFG_GCCISH_CFLAGS_x86_64-unknown-nacl := -Wall -g -fPIC -D_YUGA_LITTLE_ENDIAN=1 -D_YUGA_BIG_ENDIAN=0
CFG_GCCISH_CXXFLAGS_x86_64-unknown-nacl := -fno-rtti
CFG_GCCISH_LINK_FLAGS_x86_64-unknown-nacl := -static -fPIC -ldl -pthread  -lrt -g
CFG_GCCISH_DEF_FLAG_x86_64-unknown-nacl := -Wl,--export-dynamic,--dynamic-list=
CFG_GCCISH_PRE_LIB_FLAGS_x86_64-unknown-nacl := -Wl,-no-whole-archive
CFG_GCCISH_POST_LIB_FLAGS_x86_64-unknown-nacl :=
CFG_DEF_SUFFIX_x86_64-unknown-nacl := .x86_64.nacl.def
CFG_INSTALL_NAME_x86_64-unknown-nacl =
CFG_LIBUV_LINK_FLAGS_x86_64-unknown-nacl = -lnacl_io
CFG_LLVM_BUILD_ENV_x86_64-unknown-nacl="CXXFLAGS=-fno-omit-frame-pointer"
CFG_EXE_SUFFIX_x86_64-unknown-nacl = .nexe
CFG_WINDOWSY_x86_64-unknown-nacl :=
CFG_UNIXY_x86_64-unknown-nacl := 1
CFG_NACLY_x86_64-unknown-nacl := 1
CFG_PATH_MUNGE_x86_64-unknown-nacl := true
CFG_LDPATH_x86_64-unknown-nacl :=
CFG_RUN_x86_64-unknown-nacl=$(2)
CFG_RUN_TARG_x86_64-unknown-nacl=$(call CFG_RUN_x86_64-unknown-nacl,,$(2))
SHARED_LIBS_DISABLED_x86_64-unknown-nacl := 1
RUSTC_FLAGS_x86_64-unknown-nacl:=
RUSTC_CROSS_FLAGS_x86_64-unknown-nacl=-C cross-path=$(CFG_NACL_CROSS_PATH) --cfg "target_libc=\"newlib\"" -L $(NACL_TC_PATH)/x86_64-nacl/lib64 -L $(CFG_NACL_CROSS_PATH)/lib/newlib_x86_64/Release
CFG_GNU_TRIPLE_x86_64-unknown-nacl := x86_64-unknown-nacl
