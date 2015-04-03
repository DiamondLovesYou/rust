/*
 * Copyright (c) 2014 The Native Client Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */
#ifndef GLIBCEMU_SYS_FCNTL_H
#define GLIBCEMU_SYS_FCNTL_H 1

#include_next <sys/fcntl.h>

#include <sys/cdefs.h>

__BEGIN_DECLS

/* NaCl's fcntl doesn't include creat() declaration
 * TOOD(sbc): remove this once this gets fixed:
 * https://code.google.com/p/nativeclient/issues/detail?id=3945
 */
int creat(const char *pathname, mode_t mode);

__END_DECLS

#endif  /* GLIBCEMU_SYS_FCNTL_H */
