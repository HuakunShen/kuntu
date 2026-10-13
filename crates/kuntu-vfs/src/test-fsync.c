#include <sys/stat.h>
#include <fcntl.h>
#include <unistd.h>
#include <stdlib.h>
#include <stdio.h>
#include <dlfcn.h>
static int observed_fsync(int fd) {
 struct stat s, target;
 const char *dir=getenv("KUNTU_VFS_SYNC_DIR");
 const char *log=getenv("KUNTU_VFS_SYNC_LOG");
 if(dir && log && !fstat(fd,&s) && !stat(dir,&target) && S_ISDIR(s.st_mode) && s.st_dev==target.st_dev && s.st_ino==target.st_ino) {
   FILE *f=fopen(log,"a");
   if(f) { fprintf(f,"%d\n",!!(fcntl(fd,F_GETFD)&FD_CLOEXEC)); fclose(f); }
 }
#ifdef __APPLE__
 return fsync(fd);
#else
 int (*real_fsync)(int)=(int(*)(int))dlsym(RTLD_NEXT,"fsync");
 return real_fsync(fd);
#endif
}
#ifdef __APPLE__
__attribute__((used)) static struct { const void *replacement,*original; }
 hook __attribute__((section("__DATA,__interpose")))={(const void*)observed_fsync,(const void*)fsync};
#else
int fsync(int fd) { return observed_fsync(fd); }
#endif
