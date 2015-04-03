#ifndef GLIBCEMU_SYS_ERRNO_H
#define GLIBCEMU_SYS_ERRNO_H 1
#include_next <sys/errno.h>

#ifndef __set_errno
#define __set_errno(x) (errno = (x))
#endif

#endif
