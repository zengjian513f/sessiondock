#include <linux/bpf.h>
#define SEC(n) __attribute__((section(n), used))
#define U(name, value) int (*name)[value]
#define T(name, value) value *name
#define CORE __attribute__((preserve_access_index))
typedef unsigned long long u64;
typedef unsigned int u32;
struct super_block { unsigned long s_magic; u32 s_dev; void *s_bdev; } CORE;
struct inode { unsigned short i_mode; struct super_block *i_sb; } CORE;
struct file { struct inode *f_inode; } CORE;
struct task_struct { struct task_struct *group_leader; u64 start_boottime; } CORE;
static void *(*lookup)(void *, const void *) = (void *)BPF_FUNC_map_lookup_elem;
static long (*update)(void *, const void *, const void *, u64) = (void *)BPF_FUNC_map_update_elem;
static long (*remove_key)(void *, const void *) = (void *)BPF_FUNC_map_delete_elem;
static u64 (*pid_tid)(void) = (void *)BPF_FUNC_get_current_pid_tgid;
static u64 (*uid_gid)(void) = (void *)BPF_FUNC_get_current_uid_gid;
static u64 (*now)(void) = (void *)BPF_FUNC_ktime_get_ns;
static void *(*task)(void) = (void *)BPF_FUNC_get_current_task;
static long (*read_kernel)(void *, u32, const void *) = (void *)BPF_FUNC_probe_read_kernel;
#define READ(dst, src) read_kernel(&(dst), sizeof(dst), __builtin_preserve_access_index(&(src)))
struct config { u64 start_ns; u64 stop_ns; u32 uid; u32 padding; };
volatile struct config config = {};
struct key { u64 start_ns; u64 device; u64 generation; u32 pid; u32 kind; };
struct pending { u64 device; u32 depth; u32 kind; };
struct { U(type, BPF_MAP_TYPE_HASH); U(max_entries, 32768); T(key, struct key); T(value, u64); } totals SEC(".maps");
struct { U(type, BPF_MAP_TYPE_HASH); U(max_entries, 4096); T(key, u64); T(value, struct pending); } inflight SEC(".maps");
struct { U(type, BPF_MAP_TYPE_ARRAY); U(max_entries, 1); T(key, u32); T(value, u64); } losses SEC(".maps");
static __attribute__((always_inline)) void loss(void) { u32 z=0; u64 *p=lookup(&losses,&z); if(p)__sync_fetch_and_add(p,1); }
static __attribute__((always_inline)) int enabled(void) { return (u32)uid_gid()==config.uid && now()<config.stop_ns; }
static __attribute__((always_inline)) int account(u32 kind, u64 device, long ret) {
 if(ret<=0 || !enabled())return 0;
 struct task_struct *t=task(), *leader=0; u64 start=0;
 if(READ(leader,t->group_leader) || !leader || READ(start,leader->start_boottime)){loss();return 0;}
 u64 timestamp=now(); if(timestamp<config.start_ns || timestamp>=config.stop_ns)return 0;
 struct key k={.start_ns=start,.device=device,.generation=(timestamp-config.start_ns)/2000000000ULL,.pid=pid_tid()>>32,.kind=kind};
 u64 *v=lookup(&totals,&k);
 if(v){__sync_fetch_and_add(v,(u64)ret);return 0;}
 u64 initial=ret;
 if(update(&totals,&k,&initial,BPF_NOEXIST)) {
  v=lookup(&totals,&k); if(v)__sync_fetch_and_add(v,(u64)ret);else loss();
 }
 return 0;
}
static __attribute__((always_inline)) int enter(struct file *f,u32 write) {
 if(!enabled())return 0;
 u64 tid=pid_tid(); struct pending *p=lookup(&inflight,&tid);
 if(p){p->depth++;return 0;}
 struct pending next={.depth=1,.kind=99};
 struct inode *inode=0; struct super_block *sb=0; unsigned short mode=0; unsigned long magic=0; void *bdev=0; u32 dev=0;
 if(!READ(inode,f->f_inode) && inode && !READ(mode,inode->i_mode) && (mode&0170000)==0100000 && !READ(sb,inode->i_sb) && sb && !READ(magic,sb->s_magic) && !READ(bdev,sb->s_bdev) && !READ(dev,sb->s_dev) && (magic==0x6969 || bdev)) {
  next.device=dev; next.kind=2+write+(magic==0x6969?2:0);
 }
 if(update(&inflight,&tid,&next,BPF_ANY))loss();
 return 0;
}
static __attribute__((always_inline)) int exit_file(long ret) {
 u64 tid=pid_tid(); struct pending *p=lookup(&inflight,&tid); if(!p)return 0;
 if(p->depth>1){p->depth--;return 0;}
 u32 kind=p->kind; u64 dev=p->device; remove_key(&inflight,&tid);
 if(kind!=99)account(kind,dev,ret); return 0;
}
#define FILE_PROBE(name, write, args) \
 SEC("fentry/" #name) int name##_enter(u64 *ctx){return enter((void *)ctx[0],write);} \
 SEC("fexit/" #name) int name##_exit(u64 *ctx){return exit_file((long)ctx[args]);}
FILE_PROBE(vfs_read,0,4)
FILE_PROBE(vfs_write,1,4)
FILE_PROBE(vfs_readv,0,5)
FILE_PROBE(vfs_writev,1,5)
FILE_PROBE(vfs_iter_read,0,4)
FILE_PROBE(vfs_iter_write,1,4)
SEC("fexit/tcp_sendmsg") int tcp_send(u64 *ctx){return account(0,0,(int)ctx[3]);}
SEC("fexit/tcp_recvmsg") int tcp_recv(u64 *ctx){if(ctx[3]&2)return 0;return account(1,0,(int)ctx[5]);}
SEC("tracepoint/sched/sched_process_exit") int process_exit(void *ctx){u64 tid=pid_tid();remove_key(&inflight,&tid);return 0;}
char LICENSE[] SEC("license")="GPL";
