/*
 * Copyright 2015 The Native Client Authors. All rights reserved.
 * Use of this source code is governed by a BSD-style license that can be
 * found in the LICENSE file.
 */

#ifndef GLIBCEMU_SYS_FCNTL_H
#define GLIBCEMU_SYS_FCNTL_H 1

#include_next <sys/fcntl.h>

#include <sys/cdefs.h>
#include <sys/types.h>

__BEGIN_DECLS

#define F_ULOCK 0
#define F_LOCK  1
#define F_TLOCK 2
int lockf(int fd, int command, off_t size);

__END_DECLS

#endif
