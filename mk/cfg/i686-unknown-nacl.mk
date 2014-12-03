
NACL_TC_PATH := $(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_x86_newlib

# i686-unknown-nacl (non-portable)
CC_i686-unknown-nacl=$(NACL_TC_PATH)/bin/i686-nacl-gcc
CXX_i686-unknown-nacl=$(NACL_TC_PATH)/bin/i686-nacl-g++
CPP_i686-unknown-nacl=$(CXX_i686-unknown-nacl) -E
AR_i686-unknown-nacl=$(NACL_TC_PATH)/bin/i686-nacl-ar
CFG_LIB_NAME_i686-unknown-nacl=lib$(1).so
CFG_STATIC_LIB_NAME_i686-unknown-nacl=lib$(1).a
CFG_LIB_GLOB_i686-unknown-nacl=lib$(1)-*.so
CFG_LIB_DSYM_GLOB_i686-unknown-nacl=lib$(1)-*.dylib.dSYM
CFG_GCCISH_CFLAGS_i686-unknown-nacl := -Wall -Werror -g -fPIC -D_YUGA_LITTLE_ENDIAN=1 -D_YUGA_BIG_ENDIAN=0
CFG_GCCISH_CXXFLAGS_i686-unknown-nacl := -fno-rtti
CFG_GCCISH_LINK_FLAGS_i686-unknown-nacl := -static -fPIC -ldl -pthread  -lrt -g
CFG_GCCISH_DEF_FLAG_i686-unknown-nacl := -Wl,--export-dynamic,--dynamic-list=
CFG_GCCISH_PRE_LIB_FLAGS_i686-unknown-nacl := -Wl,-no-whole-archive
CFG_GCCISH_POST_LIB_FLAGS_i686-unknown-nacl :=
CFG_DEF_SUFFIX_i686-unknown-nacl := .i686.nacl.def
CFG_INSTALL_NAME_i686-unknown-nacl =
CFG_LIBUV_LINK_FLAGS_i686-unknown-nacl = -lnacl_io
CFG_LLVM_BUILD_ENV_i686-unknown-nacl="CXXFLAGS=-fno-omit-frame-pointer"
CFG_EXE_SUFFIX_i686-unknown-nacl = .nexe
CFG_WINDOWSY_i686-unknown-nacl :=
CFG_UNIXY_i686-unknown-nacl := 1
CFG_NACLY_i686-unknown-nacl := 1
CFG_PATH_MUNGE_i686-unknown-nacl := true
CFG_LDPATH_i686-unknown-nacl :=
CFG_RUN_i686-unknown-nacl=$(2)
CFG_RUN_TARG_i686-unknown-nacl=$(call CFG_RUN_i686-unknown-nacl,,$(2))
SHARED_LIBS_DISABLED_i686-unknown-nacl := 1
RUSTC_FLAGS_i686-unknown-nacl:=
RUSTC_CROSS_FLAGS_i686-unknown-nacl=-C cross-path=$(CFG_NACL_CROSS_PATH) --cfg "target_libc=\"newlib\"" -L $(NACL_TC_PATH)/x86_64-nacl/lib64 -L $(CFG_NACL_CROSS_PATH)/lib/newlib_x86_32/Release
CFG_GNU_TRIPLE_i686-unknown-nacl := i686-unknown-nacl
