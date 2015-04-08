# le32-unknown-nacl (portable, PNaCl)
CROSS_PREFIX_le32-unknown-nacl:=$(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_pnacl/bin/pnacl-
CC_le32-unknown-nacl=clang
CXX_le32-unknown-nacl=clang++
CPP_le32-unknown-nacl=$(CXX_le32-unknown-nacl) -E
AR_le32-unknown-nacl=ar
CFG_LIB_NAME_le32-unknown-nacl=lib$(1).so
CFG_STATIC_LIB_NAME_le32-unknown-nacl=lib$(1).a
CFG_LIB_GLOB_le32-unknown-nacl=lib$(1)-*.so
CFG_LIB_DSYM_GLOB_le32-unknown-nacl=lib$(1)-*.dylib.dSYM
CFG_CFLAGS_le32-unknown-nacl := -Wall -Wno-unused-variable -Wno-unused-value -I$(CFG_NACL_CROSS_PATH)/include -I$(CFG_NACL_CROSS_PATH)/include/pnacl -D_POSIX_READER_WRITER_LOCKS -D_YUGA_LITTLE_ENDIAN=1 -D_YUGA_BIG_ENDIAN=0 -Os
CFG_CXXFLAGS_le32-unknown-nacl := -stdlib=libc++ $(CFG_CFLAGS_le32-unknown-nacl)
CFG_GCCISH_CFLAGS_le32-unknown-nacl := $(CFG_CFLAGS_le32-unknown-nacl)
CFG_GCCISH_CXXFLAGS_le32-unknown-nacl := $(CFG_CXXFLAGS_le32-unknown-nacl)
CFG_GCCISH_LINK_FLAGS_le32-unknown-nacl := -static -pthread -lm
CFG_GCCISH_DEF_FLAG_le32-unknown-nacl := -Wl,--export-dynamic,--dynamic-list=
CFG_GCCISH_PRE_LIB_FLAGS_le32-unknown-nacl := -Wl,-no-whole-archive
CFG_GCCISH_POST_LIB_FLAGS_le32-unknown-nacl :=
CFG_DEF_SUFFIX_le32-unknown-nacl := .le32.nacl.def
CFG_INSTALL_NAME_le32-unknown-nacl =
CFG_LIBUV_LINK_FLAGS_le32-unknown-nacl =
CFG_DISABLE_LIBUV_le32-unknown-nacl := 1
CFG_EXE_SUFFIX_le32-unknown-nacl = .pexe
CFG_WINDOWSY_le32-unknown-nacl :=
CFG_UNIXY_le32-unknown-nacl := 1
CFG_NACLY_le32-unknown-nacl := 1
CFG_PATH_MUNGE_le32-unknown-nacl := true
CFG_LDPATH_le32-unknown-nacl :=
CFG_RUN_le32-unknown-nacl=$(2)
CFG_RUN_TARG_le32-unknown-nacl=$(call CFG_RUN_le32-unknown-nacl,,$(2))
TOOLCHAIN_PROVIDES_COMPRT_le32-unknown-nacl := 1
# libuv uses quite a bit of C functions that newlib doesn't implement.
# Last time I tried to build libuv for newlib it was a mess.
# Obviously, this additionally disables libgreen.
DISABLED_CRATES_le32-unknown-nacl := rustuv green
SHARED_LIBS_DISABLED_le32-unknown-nacl := 1
RUNTIME_CFLAGS_le32-unknown-nacl:= -I$(CFG_NACL_CROSS_PATH)/include/pnacl
RUNTIME_DISABLE_ASM_le32-unknown-nacl := 1
RUSTC_FLAGS_le32-unknown-nacl:=
RUSTC_CROSS_FLAGS_le32-unknown-nacl=-C cross-path=$(CFG_NACL_CROSS_PATH) --cfg "target_libc=\"newlib\"" -L $(CFG_NACL_CROSS_PATH)/lib/pnacl/Release -L $(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_pnacl/lib/clang/3.4/lib/le32-nacl -L $(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_pnacl/lib/clang/3.5.0/lib/le32-nacl -L $(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_pnacl/lib/clang/3.6.0/lib/le32-nacl -L $(S)src/etc/third-party/nacl-spawn/lib
CFG_GNU_TRIPLE_le32-unknown-nacl := le32-unknown-nacl
