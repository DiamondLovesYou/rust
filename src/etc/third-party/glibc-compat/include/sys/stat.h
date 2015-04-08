/*
 * Copyright 2015 The Native Client Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

#ifndef GLIBCEMU_SYS_STAT_H
#define GLIBCEMU_SYS_STAT_H 1

#include_next <sys/stat.h>

#ifdef __cplusplus
extern "C" {
#endif

mode_t umask(mode_t mask);

#ifdef __cplusplus
}
#endif

#endif
