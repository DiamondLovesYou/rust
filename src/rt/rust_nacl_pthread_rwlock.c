// Copyright 2012-2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// ----------------------------------------------------------------------
// Super quick-and-dirty implementation of pthread_rwlock for NaCl newlib
// macton@insomniacgames.com
// ----------------------------------------------------------------------
// Modified for use in Rust by Richard Diamond.
// ----------------------------------------------------------------------

// ----------------------------------------------------------------------
// Obvious differences between this and an actual implementation.
//   - Multiple reads can be aquired so long as no write is aquired
//   - Multiple writes can be aquired so long as no read is aquired
//     - A constraint of one write lock (or at least contrained to a
//       single thread) could be added, but it's not generally
//       used (by me) in a way where that'd make a difference.
//   - None of the typical pthread error checking is done
// ----------------------------------------------------------------------

// You know what would be better here? If there was an actual implementation
// of the gcc __atomic builtins available. Specifically, some further (non-full
// baerrier) optimizations could be done using the ACQUIRE/RELEASE mode.
// ... sadly (1) they aren't available that I can tell.
//           (2) even if they were, the reference implementation is based on the
//               __sync primitives anyway.

#ifdef __native_client__

#include <stdint.h>
#include <errno.h>

// Internal because rust doesn't need these.
typedef struct pthread_rwlock_t pthread_rwlock_t;

struct pthread_rwlock_t {
  int32_t m_ReadCount;
  int32_t m_WriteCount;
} __attribute__ ((aligned(4)));

enum
{
  kRwLockSpinCount = 32,
  kRwLockSleepMs   = 8
};

int pthread_yield( void )
{
  // instead of sched_yield() use nanosleep
  // Reference: libatomic_ops http://www.hpl.hp.com/research/linux/atomic_ops/index.php4
  // See also: https://groups.google.com/d/topic/native-client-discuss/khBNKzdDZ0w/discussion

  struct timespec ts;
  ts.tv_sec  = 0;
  ts.tv_nsec = 1000000 * kRwLockSleepMs;
  nanosleep(&ts, 0);

  return (0);
}
int pthread_rwlock_tryrdlock(pthread_rwlock_t* lock);
int pthread_rwlock_rdlock(pthread_rwlock_t* lock) {
  while (1) {
    const int r = pthread_rwlock_tryrdlock(lock);

    if (r != EBUSY) { return r; }

    pthread_yield();
  }
}
int pthread_rwlock_trywrlock(pthread_rwlock_t* lock);
int pthread_rwlock_wrlock(pthread_rwlock_t* lock) {
  while (1) {
    const int r = pthread_rwlock_trywrlock(lock);

    if (r != EBUSY) { return r; }

    pthread_yield();
  }
}

int pthread_rwlock_unlock( pthread_rwlock_t* lock )
{
  int32_t*  read_count  = &lock->m_ReadCount;
  int32_t*  write_count = &lock->m_WriteCount;
  int32_t   read_lock;
  int32_t   write_lock;

  // One of these two locks will be stable. So we're really only looking
  // out for quick fluctuations of the other type of lock trying to be
  // aquired.

  while (1)
  {
    for (int i=0;i<kRwLockSpinCount;i++)
    {
      read_lock  = __sync_fetch_and_add( read_count,  0 );
      write_lock = __sync_fetch_and_add( write_count, 0 );

      if (( read_lock > 0 ) && ( write_lock == 0 ))
      {
        __sync_fetch_and_sub( read_count, 1 );
        return (0);
      }

      if (( write_lock > 0 ) && ( read_lock == 0 ))
      {
        __sync_fetch_and_sub( write_count, 1 );
        return (0);
      }
    }
    pthread_yield();
  }
}

int pthread_rwlock_destroy(pthread_rwlock_t* lock) {
  (void)lock;
  return 0;
}
int pthread_rwlock_tryrdlock(pthread_rwlock_t* lock) {
  int32_t*  read_count  = &lock->m_ReadCount;
  int32_t*  write_count = &lock->m_WriteCount;
  int32_t   write_lock;

  __sync_fetch_and_add(read_count, 1);
  for (int i = 0; i < kRwLockSpinCount; i++) {
      write_lock = __sync_fetch_and_add(write_count, 0);
      if (write_lock == 0) {
        return (0);
      }
  }
  __sync_fetch_and_sub(read_count, 1);
  return EBUSY;
}
int pthread_rwlock_trywrlock(pthread_rwlock_t* lock) {
  int32_t*  read_count  = &lock->m_ReadCount;
  int32_t*  write_count = &lock->m_WriteCount;
  int32_t   read_lock;

  __sync_fetch_and_add(write_count, 1 );
  for (int i = 0; i < kRwLockSpinCount; i++) {
    read_lock = __sync_fetch_and_add(read_count, 0 );
    if (read_lock == 0) {
      return (0);
    }
  }
  __sync_fetch_and_sub(write_count, 1 );
  return (EBUSY);
}

#endif
