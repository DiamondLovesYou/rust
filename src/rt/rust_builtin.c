// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

#include <stdint.h>
#include <time.h>
#include <string.h>
#include <assert.h>
#include <stdlib.h>

#if !defined(__WIN32__)
#include <sys/time.h>
#include <sys/types.h>
#include <dirent.h>
#include <signal.h>
#include <unistd.h>
#include <pthread.h>

#if _POSIX_C_SOURCE < 1
#include <errno.h>
#endif

#else
#include <windows.h>
#include <wincrypt.h>
#include <stdio.h>
#include <tchar.h>
#endif

#ifdef __APPLE__
#include <TargetConditionals.h>
#include <mach/mach_time.h>

#if !(TARGET_OS_IPHONE)
#include <crt_externs.h>
#endif
#endif

/* Foreign builtins. */
//include valgrind.h after stdint.h so that uintptr_t is defined for msys2 w64
#include "valgrind/valgrind.h"

#ifdef __APPLE__
#if (TARGET_OS_IPHONE)
extern char **environ;
#endif
#endif

#if defined(__FreeBSD__) || defined(__linux__) || defined(__ANDROID__) \
  || defined(__DragonFly__) || defined(__OpenBSD__)
extern char **environ;
#endif

#if defined(__WIN32__)
char**
rust_env_pairs() {
    return 0;
}
#else
char**
rust_env_pairs() {
#if defined(__APPLE__) && !(TARGET_OS_IPHONE)
    char **environ = *_NSGetEnviron();
#endif
    return environ;
}
#endif

char*
#if defined(__WIN32__)
rust_list_dir_val(WIN32_FIND_DATA* entry_ptr) {
    return entry_ptr->cFileName;
}
#else
rust_list_dir_val(struct dirent* entry_ptr) {
    return entry_ptr->d_name;
}
#endif

#ifndef _WIN32

DIR*
rust_opendir(char *dirname) {
    return opendir(dirname);
}

int
rust_dirent_t_size() {
    return sizeof(struct dirent);
}

int
rust_readdir_r(DIR *dirp, struct dirent *entry, struct dirent **result) {
#if _POSIX_C_SOURCE < 1
    /// C disgusts me. Sigh.
    /// This is needed for newlib on PNaCl/NaCl.
    if(result == NULL || entry == NULL || dirp == NULL) {
        errno = EBADF;
        return EBADF;
    }

    errno = 0;
    struct dirent* next_entry = readdir(dirp);
    if(next_entry == NULL) {
        *result = NULL;
    } else {
        memcpy(entry, next_entry, rust_dirent_t_size());
        *result = next_entry;
    }
    return 0;
#else
    return readdir_r(dirp, entry, result);
#endif
}

#else

void
rust_opendir() {
}

void
rust_readdir() {
}

void
rust_dirent_t_size() {
}

#endif

uintptr_t
rust_running_on_valgrind() {
    return RUNNING_ON_VALGRIND;
}

#if defined(__WIN32__)
int
get_num_cpus() {
    SYSTEM_INFO sysinfo;
    GetSystemInfo(&sysinfo);

    return (int) sysinfo.dwNumberOfProcessors;
}
#elif defined(__BSD__)
int
get_num_cpus() {
    /* swiped from http://stackoverflow.com/questions/150355/
       programmatically-find-the-number-of-cores-on-a-machine */

    unsigned int numCPU;
    int mib[4];
    size_t len = sizeof(numCPU);

    /* set the mib for hw.ncpu */
    mib[0] = CTL_HW;
    mib[1] = HW_AVAILCPU;  // alternatively, try HW_NCPU;

    /* get the number of CPUs from the system */
    sysctl(mib, 2, &numCPU, &len, NULL, 0);

    if( numCPU < 1 ) {
        mib[1] = HW_NCPU;
        sysctl( mib, 2, &numCPU, &len, NULL, 0 );

        if( numCPU < 1 ) {
            numCPU = 1;
        }
    }
    return numCPU;
}
#elif defined(__GNUC__)
int
get_num_cpus() {
    return sysconf(_SC_NPROCESSORS_ONLN);
}
#endif

uintptr_t
rust_get_num_cpus() {
    return get_num_cpus();
}

unsigned int
rust_valgrind_stack_register(void *start, void *end) {
  return VALGRIND_STACK_REGISTER(start, end);
}

void
rust_valgrind_stack_deregister(unsigned int id) {
  VALGRIND_STACK_DEREGISTER(id);
}

#if defined(__WIN32__)

void
rust_unset_sigprocmask() {
    // empty stub for windows to keep linker happy
}

#else

void
rust_unset_sigprocmask() {
    // this can't be safely converted to rust code because the
    // representation of sigset_t is platform-dependent
    sigset_t sset;
    sigemptyset(&sset);
    sigprocmask(SIG_SETMASK, &sset, NULL);
}

#endif

#if defined(__DragonFly__)
#include <errno.h>
// In DragonFly __error() is an inline function and as such
// no symbol exists for it.
int *__dfly_error(void) { return __error(); }
#endif

#if defined(__OpenBSD__)
#include <sys/param.h>
#include <sys/sysctl.h>
#include <limits.h>

const char * rust_current_exe() {
    static char *self = NULL;

    if (self == NULL) {
        int mib[4];
        char **argv = NULL;
        size_t argv_len;

        /* initialize mib */
        mib[0] = CTL_KERN;
        mib[1] = KERN_PROC_ARGS;
        mib[2] = getpid();
        mib[3] = KERN_PROC_ARGV;

        /* request KERN_PROC_ARGV size */
        argv_len = 0;
        if (sysctl(mib, 4, NULL, &argv_len, NULL, 0) == -1)
            return (NULL);

        /* allocate size */
        if ((argv = malloc(argv_len)) == NULL)
            return (NULL);

        /* request KERN_PROC_ARGV */
        if (sysctl(mib, 4, argv, &argv_len, NULL, 0) == -1) {
            free(argv);
            return (NULL);
        }

        /* get realpath if possible */
        if ((argv[0] != NULL) && ((*argv[0] == '.') || (*argv[0] == '/')
                                || (strstr(argv[0], "/") != NULL)))

            self = realpath(argv[0], NULL);
        else
            self = NULL;

        /* cleanup */
        free(argv);
    }

    return (self);
}
#endif

#ifdef __native_client__
#undef __arm__
#include <unwind.h>

#define STUB \
    static const char MSG1[] = "ABORT: ";     \
    static const char MSG2[] = " called!";    \
    write(2, MSG1, sizeof(MSG1) - 1);         \
    write(2, __func__, sizeof(__func__) - 1); \
    write(2, MSG2, sizeof(MSG2) - 1);         \
    abort()

void __pnacl_eh_sjlj_Unwind_DeleteException(struct _Unwind_Exception*);
_Unwind_Reason_Code __pnacl_eh_sjlj_Unwind_RaiseException(struct _Unwind_Exception*);
_Unwind_Reason_Code _Unwind_RaiseException(struct _Unwind_Exception *e) {
    return __pnacl_eh_sjlj_Unwind_RaiseException(e);
}
void _Unwind_DeleteException(struct _Unwind_Exception *e) {
    __pnacl_eh_sjlj_Unwind_DeleteException(e);
}

void _Unwind_PNaClSetResult0(struct _Unwind_Context *c, _Unwind_Word w) {
    STUB;
}
void _Unwind_PNaClSetResult1(struct _Unwind_Context *c, _Unwind_Word w) {
    STUB;
}
_Unwind_Ptr _Unwind_GetIP(struct _Unwind_Context *c) {
    STUB;
}
void _Unwind_SetIP(struct _Unwind_Context *c, _Unwind_Ptr p) {
    STUB;
}
void *_Unwind_GetLanguageSpecificData(struct _Unwind_Context *c) {
    STUB;
}
_Unwind_Ptr _Unwind_GetRegionStart(struct _Unwind_Context *c) {
    STUB;
}
_Unwind_Reason_Code _Unwind_Resume_or_Rethrow(struct _Unwind_Exception *e) {
    STUB;
}
_Unwind_Ptr _Unwind_GetIPInfo(struct _Unwind_Context *c, int *i) {
    STUB;
}
_Unwind_Ptr _Unwind_GetTextRelBase(struct _Unwind_Context *c) {
    STUB;
}
_Unwind_Ptr _Unwind_GetDataRelBase(struct _Unwind_Context *c) {
    STUB;
}

#endif

//
// Local Variables:
// mode: C++
// fill-column: 78;
// indent-tabs-mode: nil
// c-basic-offset: 4
// buffer-file-coding-system: utf-8-unix
// End:
//
