/* Descriptor-relative I/O for bundled SQLite's Unix VFS. Unknown paths use
 * the original syscalls. Our synthetic paths never leave the opened home.
 * SQLite owns byte-range DB/WAL/SHM locking and recovery, unchanged. */
#include <sys/types.h>
#include <sys/stat.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
typedef struct sqlite3_vfs sqlite3_vfs;
typedef void (*syscall_ptr)(void);
struct sqlite3_vfs {
 int iVersion, szOsFile, mxPathname; sqlite3_vfs *pNext; const char *zName;
 void *pAppData;
 int (*xOpen)(sqlite3_vfs*, const char*, void*, int, int*);
 int (*xDelete)(sqlite3_vfs*,const char*,int);
 int (*xAccess)(sqlite3_vfs*,const char*,int,int*);
 int (*xFullPathname)(sqlite3_vfs*,const char*,int,char*);
 void *(*xDlOpen)(sqlite3_vfs*,const char*);
 void (*xDlError)(sqlite3_vfs*,int,char*);
 void (*(*xDlSym)(sqlite3_vfs*,void*,const char*))(void);
 void (*xDlClose)(sqlite3_vfs*,void*);
 int (*xRandomness)(sqlite3_vfs*,int,char*);
 int (*xSleep)(sqlite3_vfs*,int);
 int (*xCurrentTime)(sqlite3_vfs*,double*);
 int (*xGetLastError)(sqlite3_vfs*,int,char*);
 int (*xCurrentTimeInt64)(sqlite3_vfs*,long long*);
 int (*xSetSystemCall)(sqlite3_vfs*,const char*,syscall_ptr);
 syscall_ptr (*xGetSystemCall)(sqlite3_vfs*,const char*);
 const char *(*xNextSystemCall)(sqlite3_vfs*,const char*);
};
extern sqlite3_vfs *sqlite3_vfs_find(const char*);
extern int sqlite3_vfs_register(sqlite3_vfs*, int);
extern int sqlite3_initialize(void);
static sqlite3_vfs kv;
static int parse(const char *p, int *fd, const char **leaf) {
 if (strncmp(p,"/kuntu-fd/",10)) return 0;
 char *end; long n=strtol(p+10,&end,10);
 if (n<0 || n>2147483647 || *end!='/' || !end[1] || strchr(end+1,'/') || strstr(end+1,"..")) { errno=EINVAL; return -1; }
 *fd=(int)n; *leaf=end+1; return 1;
}
static int ko(const char *p,int flags,int mode) {
 int fd; const char *leaf; int r=parse(p,&fd,&leaf);
 if(r<0)return -1;
 if(r==0) return open(p,flags,mode);
 int f=openat(fd,leaf,flags|O_NOFOLLOW|O_CLOEXEC,0600);
 if(f>=0) { struct stat s; if(fstat(f,&s) || !S_ISREG(s.st_mode) || s.st_uid!=getuid() || s.st_nlink!=1 || (s.st_mode&0777)!=0600) {close(f);errno=EACCES;return -1;} }
 return f;
}
static int ks(const char *p,struct stat *s) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?fstatat(fd,l,s,AT_SYMLINK_NOFOLLOW):stat(p,s);}
static int kl(const char *p,struct stat *s) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?fstatat(fd,l,s,AT_SYMLINK_NOFOLLOW):lstat(p,s);}
static int ka(const char *p,int m) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?faccessat(fd,l,m,AT_SYMLINK_NOFOLLOW):access(p,m);}
static int ku(const char *p) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?unlinkat(fd,l,0):unlink(p);}
static ssize_t kr(const char *p,char *b,size_t n) {int fd;const char*l;int r=parse(p,&fd,&l);if(r<0)return -1;return r?readlinkat(fd,l,b,n):readlink(p,b,n);}
static int full(sqlite3_vfs*v,const char*p,int n,char*b) { (void)v;int fd;const char*l;if(parse(p,&fd,&l)!=1 || (int)strlen(p)>=n)return 14;strcpy(b,p);return 0; }
int kuntu_vfs_register(void) {
 if(sqlite3_initialize())return 1;
 sqlite3_vfs*u=sqlite3_vfs_find("unix");if(!u || u->iVersion<3)return 1;
 kv=*u;kv.zName="kuntu-fd";kv.pNext=0;kv.xFullPathname=full;
 if(u->xSetSystemCall(u,"open",(syscall_ptr)ko) || u->xSetSystemCall(u,"stat",(syscall_ptr)ks) || u->xSetSystemCall(u,"lstat",(syscall_ptr)kl) || u->xSetSystemCall(u,"access",(syscall_ptr)ka) || u->xSetSystemCall(u,"unlink",(syscall_ptr)ku) || u->xSetSystemCall(u,"readlink",(syscall_ptr)kr))return 1;
 return sqlite3_vfs_register(&kv,0);
}
