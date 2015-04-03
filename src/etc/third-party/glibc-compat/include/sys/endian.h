/*
 * Copyright (c) 2014 The Native Client Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

#ifndef GLIBCEMU_SYS_ENDIAN_H
#define GLIBCEMU_SYS_ENDIAN_H 1

#include <machine/endian.h>

/* At the moment NaCl only runs on little-endian machines. */
#define BYTE_ORDER LITTLE_ENDIAN

#define htole16(x) (x)
#define htole32(x) (x)
#define letoh16(x) (x)
#define letoh32(x) (x)

#endif
