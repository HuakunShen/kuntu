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
#include <stdint.h>
#include <time.h>
#ifdef __APPLE__
#include <pthread.h>
#include <mach/mach.h>
#endif
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
static int (*unix_open_directory)(const char*,int*);
/* Optional per-thread probe counters. Normal connections only take the
 * disabled branch; measurements include syscall wall time, not pure disk time. */
struct kuntu_probe_metrics { uint64_t retries, sleep_ns, io_ns, io_calls; };
static _Thread_local int measuring;
static _Thread_local struct kuntu_probe_metrics probe;
static uint64_t monotonic_ns(void) {
 struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t);
 return (uint64_t)t.tv_sec*1000000000ULL+(uint64_t)t.tv_nsec;
}
uint64_t kuntu_thread_cpu_ns(void) {
#ifdef __APPLE__
 thread_basic_info_data_t info;
 mach_msg_type_number_t count=THREAD_BASIC_INFO_COUNT;
 if(thread_info(pthread_mach_thread_np(pthread_self()),THREAD_BASIC_INFO,(thread_info_t)&info,&count)!=KERN_SUCCESS)return 0;
 return ((uint64_t)info.user_time.seconds+(uint64_t)info.system_time.seconds)*1000000000ULL+
        ((uint64_t)info.user_time.microseconds+(uint64_t)info.system_time.microseconds)*1000ULL;
#else
 struct timespec t; if(clock_gettime(CLOCK_THREAD_CPUTIME_ID,&t))return 0;
 return (uint64_t)t.tv_sec*1000000000ULL+(uint64_t)t.tv_nsec;
#endif
}
void kuntu_probe_start(void) { memset(&probe,0,sizeof(probe));measuring=1; }
void kuntu_probe_stop(struct kuntu_probe_metrics *out) { measuring=0;*out=probe; }
static ssize_t (*unix_pread)(int,void*,size_t,off_t);
static ssize_t (*unix_read)(int,void*,size_t);
static int (*unix_fstat)(int,struct stat*);
static int (*unix_sleep)(sqlite3_vfs*,int);
static ssize_t measured_pread(int fd,void *b,size_t n,off_t off) {
 if(!measuring)return unix_pread(fd,b,n,off);
 uint64_t t=monotonic_ns();ssize_t result=unix_pread(fd,b,n,off);int saved=errno;
 probe.io_ns+=monotonic_ns()-t;probe.io_calls++;errno=saved;return result;
}
static ssize_t measured_read(int fd,void *b,size_t n) {
 if(!measuring)return unix_read(fd,b,n);
 uint64_t t=monotonic_ns();ssize_t result=unix_read(fd,b,n);int saved=errno;
 probe.io_ns+=monotonic_ns()-t;probe.io_calls++;errno=saved;return result;
}
static int measured_fstat(int fd,struct stat *s) {
 if(!measuring)return unix_fstat(fd,s);
 uint64_t t=monotonic_ns();int result=unix_fstat(fd,s);int saved=errno;
 probe.io_ns+=monotonic_ns()-t;probe.io_calls++;errno=saved;return result;
}
static int measured_sleep(sqlite3_vfs *v,int us) {
 if(!measuring)return unix_sleep(v,us);
 uint64_t t=monotonic_ns();int result=unix_sleep(v,us);
 probe.retries++;probe.sleep_ns+=monotonic_ns()-t;return result;
}
#ifdef KUNTU_TEST_HOOKS
int kuntu_vfs_test_delete(const char *p) { return kv.xDelete(&kv,p,1); }
#endif
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
static int kd(const char *p,int *out) {
 int fd;const char *leaf;int r=parse(p,&fd,&leaf);
 *out=-1;
 if(r==0)return unix_open_directory(p,out);
 if(r<0)return 14;
 struct stat s;
 if(fstat(fd,&s) || !S_ISDIR(s.st_mode))return 14;
 /* SQLite closes this descriptor after its directory fsync. Keep the
  * validated home descriptor pinned, and never leak the duplicate on exec. */
 *out=fcntl(fd,F_DUPFD_CLOEXEC,0);
 return *out<0?14:0;
}
int kuntu_vfs_register(void) {
 if(sqlite3_initialize())return 1;
 sqlite3_vfs*u=sqlite3_vfs_find("unix");if(!u || u->iVersion<3)return 1;
 unix_pread=(ssize_t(*)(int,void*,size_t,off_t))u->xGetSystemCall(u,"pread");
 unix_read=(ssize_t(*)(int,void*,size_t))u->xGetSystemCall(u,"read");
 unix_fstat=(int(*)(int,struct stat*))u->xGetSystemCall(u,"fstat");
 if((unix_pread && u->xSetSystemCall(u,"pread",(syscall_ptr)measured_pread)) ||
    (unix_read && u->xSetSystemCall(u,"read",(syscall_ptr)measured_read)) ||
    (unix_fstat && u->xSetSystemCall(u,"fstat",(syscall_ptr)measured_fstat)))return 1;
 unix_sleep=u->xSleep;
 kv=*u;kv.xSleep=measured_sleep;kv.zName="kuntu-fd";kv.pNext=0;kv.xFullPathname=full;
 unix_open_directory=(int(*)(const char*,int*))u->xGetSystemCall(u,"openDirectory");
 if(!unix_open_directory || u->xSetSystemCall(u,"openDirectory",(syscall_ptr)kd))return 1;
 if(u->xSetSystemCall(u,"open",(syscall_ptr)ko) || u->xSetSystemCall(u,"stat",(syscall_ptr)ks) || u->xSetSystemCall(u,"lstat",(syscall_ptr)kl) || u->xSetSystemCall(u,"access",(syscall_ptr)ka) || u->xSetSystemCall(u,"unlink",(syscall_ptr)ku) || u->xSetSystemCall(u,"readlink",(syscall_ptr)kr))return 1;
 return sqlite3_vfs_register(&kv,0);
}
