# Copyright 2012 The Rust Project Developers. See the COPYRIGHT
# file at the top-level directory of this distribution and at
# http://rust-lang.org/COPYRIGHT.
#
# Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
# http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
# <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
# option. This file may not be copied, modified, or distributed
# except according to those terms.

ifdef CFG_ENABLE_FAST_MAKE
BINUTILS_DEPS := $(S)/.gitmodules
else

# This is just a rough approximation of binutil deps
BINUTILS_DEPS_SRC=$(call rwildcard,$(CFG_BINUTILS_SRC_DIR)lib,*cpp *hpp *h *cc)
BINUTILS_DEPS_INC=$(call rwildcard,$(CFG_BINUTILS_SRC_DIR)include,*cpp *hpp)
BINUTILS_DEPS=$(BINUTILS_DEPS_SRC) $(BINUTILS_DEPS_INC)
endif

define DEF_BINUTILS_RULES
# $(1) is the host

BINUTILS_BUILD_STAMP_$(1) = $$(CFG_BINUTILS_BUILD_DIR_$(1))/build.stamp
BINUTILS_CP_STAMP_$(1) = $$(CFG_BINUTILS_BUILD_DIR_$(1))/cp.stamp

ifneq ($(filter le32-unknown-nacl,$(CFG_TARGET)),$(CFG_TARGET))
# the user is building the PNaCl cross.

$$(BINUTILS_BUILD_STAMP_$(1)): $$(BINUTILS_DEPS)
	@$$(call E, make: nacl-binutils)
	$$(Q)$$(MAKE) -C $$(CFG_BINUTILS_BUILD_DIR_$(1))
	$$(Q)touch $$@

$$(CFG_BINUTILS_BUILD_DIR_$(1))/gold/ld-new$$(X_$(1)): $$(BINUTILS_BUILD_STAMP_$(1))
$$(CFG_BINUTILS_BUILD_DIR_$(1))/binutils/ar$$(X_$(1)): $$(BINUTILS_BUILD_STAMP_$(1))

$$(BINUTILS_CP_STAMP_$(1)):     $$(BINUTILS_BUILD_STAMP_$(1))                         \
				$$(CFG_BINUTILS_BUILD_DIR_$(1))/gold/ld-new$$(X_$(1)) \
				$$(CFG_BINUTILS_BUILD_DIR_$(1))/binutils/ar$$(X_$(1))
	@$$(call E, cp: ar/ld.gold)
	$$(Q)cp $$(CFG_BINUTILS_BUILD_DIR_$(1))/gold/ld-new$$(X_$(1)) \
		$$(TBIN2_T_$(1)_H_$(1))/le32-nacl-ld.gold$$(X_$(1))
	$$(Q)cp $$(CFG_BINUTILS_BUILD_DIR_$(1))/binutils/ar$$(X_$(1)) \
		$$(TBIN2_T_$(1)_H_$(1))/le32-nacl-ar$$(X_$(1))
	$$(Q)touch $$@

else
# the user is not building the PNaCl cross.
# in this case, we don't build binutils.

$$(BINUTILS_BUILD_STAMP_$(1)):
	@$$(call E, make: nacl-binutils (SKIPPING))
	$$(Q)touch $$@

$$(BINUTILS_CP_STAMP_$(1)):
	$$(Q)touch $$@

endif

$$(BINUTILS_STAMP_$(1)): $$(BINUTILS_CP_STAMP_$(1)) $$(BINUTILS_BUILD_STAMP_$(1))
	$$(Q)touch $$@

endef

$(foreach host,$(CFG_HOST),                      \
 $(eval $(call DEF_BINUTILS_RULES,$(host))))
