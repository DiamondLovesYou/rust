ifeq ($(CFG_OSTYPE),pc-mingw32)
  NACL_TOOLCHAIN_OS_PATH:=win
else ifeq ($(CFG_OSTYPE),apple-darwin)
  NACL_TOOLCHAIN_OS_PATH:=mac
else
  NACL_TOOLCHAIN_OS_PATH:=linux
endif

# arm-unknown-nacl (non-portable)
CC_arm-unknown-nacl=$(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_arm_newlib/bin/arm-nacl-gcc
CXX_arm-unknown-nacl=$(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_arm_newlib/bin/arm-nacl-g++
CPP_arm-unknown-nacl=$(CXX_arm-unknown-nacl) -E
AR_arm-unknown-nacl=$(CFG_NACL_CROSS_PATH)/toolchain/$(NACL_TOOLCHAIN_OS_PATH)_arm_newlib/bin/arm-nacl-ar
CFG_LIB_NAME_arm-unknown-nacl=lib$(1).so
CFG_STATIC_LIB_NAME_arm-unknown-nacl=lib$(1).a
CFG_LIB_GLOB_arm-unknown-nacl=lib$(1)-*.so
CFG_LIB_DSYM_GLOB_arm-unknown-nacl=lib$(1)-*.dylib.dSYM
CFG_GCCISH_CFLAGS_arm-unknown-nacl := -Wall -Werror -g -fPIC -D_YUGA_LITTLE_ENDIAN=1 -D_YUGA_BIG_ENDIAN=0
CFG_GCCISH_CXXFLAGS_arm-unknown-nacl := -fno-rtti
CFG_GCCISH_LINK_FLAGS_arm-unknown-nacl := -static -fPIC -ldl -pthread  -lrt -g
CFG_GCCISH_DEF_FLAG_arm-unknown-nacl := -Wl,--export-dynamic,--dynamic-list=
CFG_GCCISH_PRE_LIB_FLAGS_arm-unknown-nacl := -Wl,-no-whole-archive
CFG_GCCISH_POST_LIB_FLAGS_arm-unknown-nacl :=
CFG_DEF_SUFFIX_arm-unknown-nacl := .arm.nacl.def
CFG_INSTALL_NAME_arm-unknown-nacl =
CFG_LIBUV_LINK_FLAGS_arm-unknown-nacl = -lnacl_io
CFG_DISABLE_LIBUV_arm-unknown-nacl := 1
CFG_LLVM_BUILD_ENV_arm-unknown-nacl="CXXFLAGS=-fno-omit-frame-pointer"
CFG_EXE_SUFFIX_arm-unknown-nacl = .nexe
CFG_WINDOWSY_arm-unknown-nacl :=
CFG_UNIXY_arm-unknown-nacl := 1
CFG_PATH_MUNGE_arm-unknown-nacl := true
CFG_LDPATH_arm-unknown-nacl :=
CFG_RUN_arm-unknown-nacl=$(2)
CFG_RUN_TARG_arm-unknown-nacl=$(call CFG_RUN_arm-unknown-nacl,,$(2))
SHARED_LIBS_DISABLED_arm-unknown-nacl := 1
RUSTC_FLAGS_arm-unknown-nacl:=
RUSTC_CROSS_FLAGS_arm-unknown-nacl=-C cross-path=$(CFG_NACL_CROSS_PATH) --cfg "target_libc=\"newlib\""
CFG_GNU_TRIPLE_arm-unknown-nacl := arm-unknown-nacl
