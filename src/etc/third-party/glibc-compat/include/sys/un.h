/* olonho */
#ifndef GLIBCEMU_SYS_UN_H
#define GLIBCEMU_SYS_UN_H       1

#include <sys/socket.h>

struct sockaddr_un {
        sa_family_t     sun_family;      /* address family */
        char            sun_path[108];
};

#endif /* GLIBCEMU_SYS_UN_H */
/* end of olonho */

