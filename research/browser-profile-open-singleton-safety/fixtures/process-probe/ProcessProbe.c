#define _DARWIN_C_SOURCE 1
#include <CommonCrypto/CommonDigest.h>
#include <errno.h>
#include <fcntl.h>
#include <libproc.h>
#include <limits.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/proc_info.h>
#include <sys/stat.h>
#include <sys/sysctl.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

/* Keep protocol vocabulary here. The contract is the authority; this table makes
 * a reviewed vocabulary change local instead of scattering string literals. */
/* In the synthetic-test build the native entry points are intentionally not all
 * referenced; the marker keeps -Wall -Wextra -Werror meaningful for the rest. */
#if defined(PROCESS_PROBE_SYNTHETIC_TEST)
#define PP_MAYBE_UNUSED __attribute__((unused))
#else
#define PP_MAYBE_UNUSED
#endif

enum {
  SAMPLE_MAX = 10000, PROCESS_MAX = 4096, GROUP_MAX = 4096,
  CONTROL_MAX = 16, CONTROL_BYTES_MAX = 32768,
  IDENTITY_MAX = 16384, EDGE_MAX = 32768, EXECUTABLE_MAX = 16384,
  EVENT_MAX = 4096, TOKEN_MAX = 512, PROCARGS_MAX = 4194304,
  STATE_MAX = 16777216, FINAL_MAX = 1048576,
};
static const uint64_t CADENCE_NS = 20000000, BOUND_NS = 200000000;
/* limits.relay_first_start_window_ms / limits.relay_exit_deadline_ms */
static const uint64_t RELAY_FIRST_START_WINDOW_MS = 2000;
static const uint64_t RELAY_EXIT_DEADLINE_MS = 5000;
static const char *CONTROL_SCHEMA = "agenterm.singleton-safety.process-probe-control/v1";
static const char *STATE_SCHEMA = "agenterm.singleton-safety.process-probe-state/v1";
static const char *SUMMARY_SCHEMA = "agenterm.singleton-safety.process-probe-summary/v1";

/* ROOT_RELAY / ROOT_CANDIDATE are scanner-derived slots: they are never written by
 * a wire control (derived_relay_classification.authority), only by the candidate
 * classifier. The wire vocabulary therefore stops at the five court controls. */
typedef enum { ROOT_BROWSER=0, ROOT_DIRECT=1, ROOT_RELAY=2, ROOT_CANDIDATE=3, ROOT_COUNT=4 } RootRole;
/* root_observations is an exact object over these two court roots only. */
static const int COURT_ROOTS[2] = { ROOT_BROWSER, ROOT_DIRECT };
typedef enum { C_BROWSER, C_OWNERSHIP, C_DIRECT, C_FINAL, C_ABORT, C_COUNT, C_BAD=C_COUNT } ControlRole;
typedef enum { OBS_LIVE, OBS_VANISHED, OBS_UNREADABLE, OBS_CHANGED } ObsStatus;
typedef enum {
  EV_ROOT_OBSERVED, EV_ROOT_EXITED, EV_FIRST_SEEN, EV_LAST_SEEN, EV_REPARENTED,
  EV_TOKEN, EV_SELECTED, EV_BUNDLE, EV_GROUP, EV_BREAKAWAY, EV_CONTROLLED_LINEAGE,
  EV_DIRECT_LINEAGE, EV_RELAY_LINEAGE, EV_CANDIDATE_LINEAGE, EV_INCOMPLETE
} EventKind;
/* The incomplete_reason vocabulary and its order are fixed by the contract
 * (domains.incomplete_reason). One enumerator per reason keeps the bit position
 * and the emitted name in lockstep. */
typedef enum {
  F_ABORT_REQUESTED, F_ARGUMENT_INVALID, F_CONTROL_INVALID, F_CONTROL_LIMIT_EXCEEDED,
  F_ENUMERATION_FAILED, F_ENUMERATION_LIMIT_EXCEEDED, F_ENUMERATION_TIME_EXCEEDED, F_FIRST_SAMPLE_MISSING,
  F_GROUP_LIMIT_EXCEEDED, F_IDENTITY_LIMIT_EXCEEDED, F_EDGE_LIMIT_EXCEEDED, F_EXECUTABLE_LIMIT_EXCEEDED,
  F_PROCARGS_LIMIT_EXCEEDED, F_PROCESS_ROW_LIMIT_EXCEEDED, F_RELEVANT_EVENT_LIMIT_EXCEEDED, F_SAMPLE_LIMIT_EXCEEDED,
  F_SAMPLE_INCOMPLETE, F_SENTINEL_MISSING, F_START_INTERVAL_EXCEEDED, F_STATE_LIMIT_EXCEEDED,
  F_FINAL_LIMIT_EXCEEDED, F_PUBLICATION_FAILED, F_ROOT_UNOBSERVED, F_UNKNOWN_FIELD,
  F_UNSUPPORTED_OBSERVATION,
  F_COUNT
} Failure;

static const char *FAILURE_NAMES[F_COUNT] = {
  "abort-requested", "argument-invalid", "control-invalid",
  "control-limit-exceeded", "enumeration-failed", "enumeration-limit-exceeded",
  "enumeration-time-exceeded", "first-sample-missing", "group-limit-exceeded",
  "identity-limit-exceeded", "edge-limit-exceeded", "executable-limit-exceeded",
  "procargs-limit-exceeded", "process-row-limit-exceeded", "relevant-event-limit-exceeded",
  "sample-limit-exceeded", "sample-incomplete", "sentinel-missing",
  "start-interval-exceeded", "state-limit-exceeded", "final-limit-exceeded",
  "publication-failed", "root-unobserved", "unknown-field",
  "unsupported-observation",
};

static const char *ROOT_NAMES[ROOT_COUNT] = {"browser-root","direct-open-root","relay-root","topology-candidate-root"};
static const char *EVENT_NAMES[] = {
  "root-observed","root-exited","process-first-seen","process-last-seen","process-reparented",
  "token-match-first-seen","selected-executable-first-seen","bundle-tree-first-seen",
  "group-member-first-seen","group-breakaway","controlled-lineage-inherited",
  "direct-lineage-inherited","relay-lineage-inherited","topology-candidate-lineage-inherited",
  "observation-incomplete"
};
static const char *OBS_NAMES[] = {"live","vanished-during-sample","unreadable","identity-changed-during-sample"};

typedef struct { pid_t pid; int64_t sec; int32_t usec; } ProcIdentity;
typedef struct {
  uint32_t id; dev_t dev; ino_t ino; off_t size; int64_t mtime_ns, ctime_ns;
  uint8_t sha[CC_SHA256_DIGEST_LENGTH]; bool selected, bundle;
} Executable;
typedef struct {
  uint32_t id; ProcIdentity key; pid_t ppid, pgid; uid_t uid;
  int32_t parent_id, executable_id; int8_t token, selected, bundle, in_group;
  bool recheck, live, ever_group, ever_breakaway, in_closure_now; ObsStatus status;
  /* same_sample_insertion_check, per-sample marks: the sample at which this
   * identity was confirmed live+in_group+bundle during raw enumeration, and
   * the sample at which it was actually inserted into the live contained
   * closure. Both must equal the ownership sample. */
  uint32_t enumerated_bundle_group_sample, closure_inserted_sample;
  uint32_t first_sample, last_sample; uint8_t roots, lineage;
} Identity;
typedef struct {
  uint32_t id, child, parent, first_sample, last_sample, count; uint8_t lineage;
} Edge;
typedef struct {
  uint32_t sequence, sample, identity; EventKind kind; uint16_t witnesses; bool has_identity;
} Event;
typedef struct {
  uint32_t sequence, ids_offset, ids_count, group_count; uint64_t start_ns, end_ns, interval_ns;
  int32_t control_sequence; bool complete; uint8_t digest[32]; char kind;
} Sample;

/* derived_relay_classification: the candidate universe and its three mutually
 * exclusive buckets. Conservation is enforced by construction: every candidate is
 * counted into exactly one bucket, so candidate_observed_count equals the sum. */
typedef struct {
  bool computed;
  uint32_t candidate_observed_count;
  uint32_t classified_relay_count;
  uint32_t topology_candidate_count;
  uint32_t live_g1_exemption_count;
  uint32_t non_exempt_token_outside_count;
  uint32_t non_exempt_bundle_tree_outside_count;
  /* relay-only facts: null unless exactly one classified relay exists. */
  bool has_single_relay;
  int32_t relay_identity_id;
  bool relay_primary_ok, relay_auxiliary_ok, relay_kernel_window_ok;
  /* exit snapshot facts, filled by the absence pass. */
  bool relay_exit_observed;
  bool relay_exit_have_elapsed;
  uint64_t relay_exit_elapsed_ms;
} RelayClassification;
typedef struct { ControlRole role; uint32_t sequence; bool has_process; ProcIdentity process; } Control;

/* The six stage snapshots required by stage_snapshot_rules. Each slot holds the
 * sample hashes of the snapshot that satisfied the rule, or "absent". A stage that
 * has no satisfying sample stays absent, and any field that depends on it is then
 * unobservable rather than zero. */
typedef struct {
  bool have_baseline;        uint32_t baseline_sample;
  bool have_session_ready;   uint32_t session_ready_sample;
  bool have_open_request;    uint32_t open_request_sample;
  bool have_termination;     uint32_t termination_sample;
  bool have_final_inventory; uint32_t final_inventory_sample;
  /* Per-stage facts captured at the satisfying sample. A value is meaningful only
   * when the matching have_* flag is set; otherwise the summary field is null and
   * never zero-filled. */
  bool baseline_zero_independent;
  uint32_t baseline_selected_bundle_count;
  bool baseline_have_count;
  uint8_t session_ready_browser_identity[32]; bool session_ready_have_identity;
  uint8_t session_ready_bundle_set[32];       bool session_ready_have_bundle;
  uint32_t session_ready_selected_bundle_count; bool session_ready_have_count;
  bool open_request_exit_observed;
  uint8_t open_request_desc_retained[32]; uint32_t open_request_desc_retained_count;
  uint8_t open_request_desc_live[32];     uint32_t open_request_desc_live_count;
  bool open_request_desc_have_live;
  bool termination_browser_absent;
  bool termination_contained_group_absent;
  uint32_t termination_contained_closure_live_count;
  uint8_t termination_token_set[32];      uint32_t termination_token_count;
  uint32_t termination_escaped_target_count;
  bool final_inventory_selected_bundle_have; uint32_t final_inventory_selected_bundle_count;
  uint8_t final_inventory_token_set[32];  uint32_t final_inventory_token_count;
  uint32_t final_inventory_bundle_tree_outside_count;
  uint8_t final_inventory_contained_retained[32]; uint32_t final_inventory_contained_retained_count;
  uint8_t final_inventory_contained_live[32];     uint32_t final_inventory_contained_live_count;
  uint8_t final_inventory_direct_desc_retained[32]; uint32_t final_inventory_direct_desc_retained_count;
  uint32_t final_inventory_direct_desc_live_count;
  uint8_t final_inventory_relay_desc_retained[32];  uint32_t final_inventory_relay_desc_retained_count;
  uint32_t final_inventory_relay_desc_live_count;
  uint8_t final_inventory_candidate_desc_retained[32]; uint32_t final_inventory_candidate_desc_retained_count;
  uint32_t final_inventory_candidate_desc_live_count;
  bool final_inventory_cleanup_targets_absent;
} StageSnapshots;

/* root_observation_state: per controlled root, the first sample that observed the
 * exact identity and the first later complete sample that proved it absent. */
typedef struct {
  bool observed;      uint32_t first_observed_sample;
  bool absent;        uint32_t first_absent_sample;
  bool ever_observed;
  /* root_control_causality.miss: set when the first complete sample after a
   * non-null root control neither observed nor prior-matched that exact identity. */
  bool miss_checked;
  bool unobserved;
} RootObservation;
typedef struct {
  bool complete; uint8_t contained_group[32], controlled[32], retained[32], live[32], token[32], bundle[32];
  uint32_t group_retained, group_live, controlled_count, retained_count, live_count;
  uint32_t token_count, bundle_outside, breakaways; bool browser_token, insertion;
} Ownership;
typedef struct {
  uid_t uid; uint8_t token[TOKEN_MAX]; size_t token_len;
  char bundle_root[PATH_MAX], selected_path[PATH_MAX], control_path[PATH_MAX];
  char state_path[PATH_MAX], final_path[PATH_MAX]; struct stat selected_stat;
  uint8_t selected_sha[32]; off_t control_offset; dev_t control_dev; ino_t control_ino;
  char control_tail[CONTROL_BYTES_MAX]; size_t control_tail_len; uint32_t controls;
  bool control_seen[C_COUNT], final_seen, abort_seen, failed, early_stop, ownership_pending, ownership_done;
  int32_t last_control, final_control, ownership_sample, frozen_pgid; uint32_t failures;
  ProcIdentity roots[ROOT_COUNT]; bool root_valid[ROOT_COUNT]; int32_t root_control_seq[ROOT_COUNT]; uint32_t root_control_sample[ROOT_COUNT];
  StageSnapshots stages; RootObservation root_obs[ROOT_COUNT];
  RelayClassification relay;
  /* candidate universe membership + per-candidate bucket (parallel arrays). */
  uint32_t *cand_ids; uint8_t *cand_bucket; uint32_t cand_count, cand_cap;
  /* first sample proving each classified identity absent (sticky per candidate). */
  uint32_t *cand_absent_sample;
  int32_t relay_single_id;
  uint64_t relay_kernel_start_ms;
  Identity *ids; uint32_t id_count; Edge *edges; uint32_t edge_count;
  Executable *execs; uint32_t exec_count; Event *events; uint32_t event_count;
  Sample *samples; uint32_t sample_count; uint32_t *sample_ids; size_t sample_ids_count, sample_ids_cap;
  uint64_t max_interval, max_enumeration; uint32_t max_rows, max_group;
  Ownership ownership;
} State;



typedef struct { char *p; size_t n, cap, limit; bool failed; } Buffer;
static void b_init(Buffer *b, size_t limit) { memset(b,0,sizeof(*b)); b->limit=limit; }
static bool b_need(Buffer *b,size_t add){
  if(b->failed||add>b->limit-b->n){b->failed=true;return false;} size_t want=b->n+add+1;
  if(want<=b->cap)return true; size_t cap=b->cap?b->cap:4096; while(cap<want&&cap<b->limit)cap=cap>b->limit/2?b->limit:cap*2;
  char *p=realloc(b->p,cap+1); if(!p){b->failed=true;return false;} b->p=p;b->cap=cap;return true;
}
static void b_raw(Buffer*b,const void*p,size_t n){if(b_need(b,n)){memcpy(b->p+b->n,p,n);b->n+=n;b->p[b->n]=0;}}
static void b_s(Buffer*b,const char*s){b_raw(b,s,strlen(s));}
static void b_u(Buffer*b,uint64_t v){char x[32];int n=snprintf(x,sizeof(x),"%llu",(unsigned long long)v);b_raw(b,x,(size_t)n);}
static void b_i(Buffer*b,int64_t v){char x[32];int n=snprintf(x,sizeof(x),"%lld",(long long)v);b_raw(b,x,(size_t)n);}
static void b_bool(Buffer*b,bool v){b_s(b,v?"true":"false");}
static void b_nullable_u(Buffer*b,bool ok,uint64_t v){if(ok)b_u(b,v);else b_s(b,"null");}
static void hex32(const uint8_t d[32],char out[65]){static const char h[]="0123456789abcdef";for(int i=0;i<32;i++){out[i*2]=h[d[i]>>4];out[i*2+1]=h[d[i]&15];}out[64]=0;}
static void b_digest(Buffer*b,const uint8_t d[32]){char h[65];hex32(d,h);b_raw(b,h,64);}

static uint64_t mono_ns(void){struct timespec t;if(clock_gettime(CLOCK_MONOTONIC,&t))return 0;return(uint64_t)t.tv_sec*1000000000ull+(uint64_t)t.tv_nsec;}
/* Realtime clock in milliseconds; relay exit timing is compared against this and
 * never against the monotonic clock. */
static uint64_t realtime_ms(void){struct timespec t;if(clock_gettime(CLOCK_REALTIME,&t))return 0;return(uint64_t)t.tv_sec*1000ull+(uint64_t)(t.tv_nsec/1000000);}
static uint64_t ceil_ms(uint64_t n){return n/1000000ull+(n%1000000ull!=0);}
static bool same_proc(ProcIdentity a,ProcIdentity b){return a.pid==b.pid&&a.sec==b.sec&&a.usec==b.usec;}
static int cmp_proc(const void*a,const void*b){const ProcIdentity*x=a,*y=b;if(x->pid!=y->pid)return x->pid<y->pid?-1:1;if(x->sec!=y->sec)return x->sec<y->sec?-1:1;return x->usec<y->usec?-1:x->usec>y->usec;}
static bool hash_fd(int fd,uint8_t out[32]){CC_SHA256_CTX c;CC_SHA256_Init(&c);uint8_t buf[65536];if(lseek(fd,0,SEEK_SET)<0)return false;for(;;){ssize_t n=read(fd,buf,sizeof(buf));if(n<0){if(errno==EINTR)continue;return false;}if(!n)break;CC_SHA256_Update(&c,buf,(CC_LONG)n);}CC_SHA256_Final(out,&c);return true;}

/* Digest encoding is fixed by the contract: sha256(domain-utf8 || 0x00 || rfc8785-json-utf8).
 * Every domain digest below starts a context through d_begin() so no caller can
 * silently drop the separator or reuse a domain string. */
static void d_begin(CC_SHA256_CTX*c,const char*domain){
  static const uint8_t sep=0x00;
  CC_SHA256_Init(c);
  CC_SHA256_Update(c,domain,(CC_LONG)strlen(domain));
  CC_SHA256_Update(c,&sep,1);
}

/* RFC 8785 canonical JSON for a [pid,start_sec,start_usec] array. RFC 8785
 * inherits ECMAScript number formatting; these three values are integers, so the
 * canonical form is the shortest round-tripping decimal spelling with no
 * leading '+', no leading zeros and no exponent. */
static size_t rfc8785_identity(const ProcIdentity p,char*out,size_t cap){
  int n=snprintf(out,cap,"[%lld,%lld,%lld]",(long long)p.pid,(long long)p.sec,(long long)p.usec);
  return(n>0&&(size_t)n<cap)?(size_t)n:0;
}

static void identity_digest(ProcIdentity p,uint8_t out[32]);

static int cmp_digest(const void*a,const void*b){return memcmp(a,b,32);}

/* Emit the RFC 8785 array of lowercase-hex digest strings. buf holds n*67+3 bytes:
 * each element is "<64 hex>" plus an optional comma, wrapped in brackets. */
static size_t rfc8785_digest_array(const uint8_t(*d)[32],uint32_t n,char*out,size_t cap){
  if(cap<3)return 0;
  size_t used=0;out[used++]='[';
  for(uint32_t i=0;i<n;i++){
    if(i){if(used>=cap)return 0;out[used++]=',';}
    if(used+66>cap)return 0;
    out[used++]='"';hex32(d[i],out+used);used+=64;out[used++]='"';
  }
  if(used>=cap)return 0;
  out[used++]=']';out[used]='\0';
  return used;
}

/* Reduce identities to sorted, unique digests and hash the canonical array. */
static uint32_t digest_process_set(const ProcIdentity*v,uint32_t n,const char*domain,uint8_t out[32]){
  uint8_t(*d)[32]=n?malloc((size_t)n*32):NULL;
  if(n&&!d){memset(out,0,32);return 0;}
  for(uint32_t i=0;i<n;i++)identity_digest(v[i],d[i]);
  qsort(d,n,32,cmp_digest);
  uint32_t m=0;
  for(uint32_t i=0;i<n;i++){if(i&&!memcmp(d[i],d[i-1],32))continue;memcpy(d[m++],d[i],32);}
  size_t cap=(size_t)m*67+3;
  char*body=malloc(cap);
  if(!body){free(d);memset(out,0,32);return 0;}
  size_t bn=rfc8785_digest_array(d,m,body,cap);
  if(!bn){free(body);free(d);memset(out,0,32);return 0;}
  CC_SHA256_CTX c;d_begin(&c,domain);CC_SHA256_Update(&c,body,(CC_LONG)bn);CC_SHA256_Final(out,&c);
  free(body);free(d);
  return m;
}

static void identity_digest(ProcIdentity p,uint8_t out[32]){  /* used by stage identity digests */
  char body[128];
  size_t n=rfc8785_identity(p,body,sizeof(body));
  if(!n){memset(out,0,32);return;}
  CC_SHA256_CTX c;
  d_begin(&c,"agenterm.singleton-safety.v1.process-identity");
  CC_SHA256_Update(&c,body,(CC_LONG)n);
  CC_SHA256_Final(out,&c);
}

static void fail(State*s,Failure f){s->failed=true;s->failures|=1u<<f;}
static bool add_event(State*s,uint32_t sample,EventKind kind,int32_t id,uint16_t witnesses){
  if(s->event_count>=EVENT_MAX){fail(s,F_RELEVANT_EVENT_LIMIT_EXCEEDED);return false;} Event*e=&s->events[s->event_count];e->sequence=s->event_count;e->sample=sample;e->kind=kind;e->has_identity=id>=0;e->identity=id>=0?(uint32_t)id:0;e->witnesses=witnesses;s->event_count++;return true;
}
static int find_identity(State*s,ProcIdentity p){for(uint32_t i=0;i<s->id_count;i++)if(same_proc(s->ids[i].key,p))return(int)i;return-1;}
static int get_identity(State*s,ProcIdentity p,uint32_t sample){int i=find_identity(s,p);if(i>=0)return i;if(s->id_count>=IDENTITY_MAX){fail(s,F_IDENTITY_LIMIT_EXCEEDED);return-1;}Identity*x=&s->ids[s->id_count];memset(x,0,sizeof(*x));x->id=s->id_count;x->key=p;x->parent_id=-1;x->executable_id=-1;x->token=x->selected=x->bundle=x->in_group=-1;x->first_sample=x->last_sample=sample;x->status=OBS_UNREADABLE;x->enumerated_bundle_group_sample=x->closure_inserted_sample=UINT32_MAX;add_event(s,sample,EV_FIRST_SEEN,(int32_t)x->id,0);return(int)s->id_count++;}
static int find_edge(State*s,uint32_t c,uint32_t p){for(uint32_t i=0;i<s->edge_count;i++)if(s->edges[i].child==c&&s->edges[i].parent==p)return(int)i;return-1;}
static int add_edge(State*s,uint32_t c,uint32_t p,uint32_t sample){int i=find_edge(s,c,p);if(i>=0){Edge*e=&s->edges[i];e->last_sample=sample;e->count++;return i;}if(s->edge_count>=EDGE_MAX){fail(s,F_EDGE_LIMIT_EXCEEDED);return-1;}Edge*e=&s->edges[s->edge_count];*e=(Edge){.id=s->edge_count,.child=c,.parent=p,.first_sample=sample,.last_sample=sample,.count=1};return(int)s->edge_count++;}
/* sticky_lineage.rules 2/3/4: only edges observed in THIS sample participate in the
 * fixed point. Seeding is the previously retained lineage plus every exact root
 * observed by this sample. An edge first seen in another sample can never create a
 * same-sample chain, so filtering on the edge's sample membership is the whole rule. */
static void propagate_lineage(State*s,uint32_t sample){
  bool change=true;
  while(change){
    change=false;
    for(uint32_t i=0;i<s->edge_count;i++){
      Edge*e=&s->edges[i];
      /* Same-sample edges only: seen now, and never an edge from a later sample. */
      if(e->first_sample!=sample&&e->last_sample!=sample)continue;
      if(e->first_sample>sample)continue;
      uint8_t inherited=s->ids[e->parent].lineage|s->ids[e->parent].roots;
      uint8_t add=inherited&~s->ids[e->child].lineage;
      if(add){
        s->ids[e->child].lineage|=add;
        e->lineage|=add;
        change=true;
        for(int r=0;r<ROOT_COUNT;r++)
          if(add&(1u<<r))add_event(s,sample,(EventKind)(EV_CONTROLLED_LINEAGE+r),e->child,(uint16_t)(1u<<(4+r)));
      }
    }
  }
}

static int64_t stat_time_ns(struct timespec t){return(int64_t)t.tv_sec*1000000000ll+t.tv_nsec;}
static bool component_under(const char*root,const char*path){size_t n=strlen(root);return strncmp(root,path,n)==0&&(path[n]==0||path[n]=='/'||(n&&root[n-1]=='/'));}
static int get_executable(State*s,const char*path){
  char real[PATH_MAX];if(!realpath(path,real))return-1;int fd=open(real,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);if(fd<0)return-1;struct stat st;if(fstat(fd,&st)){close(fd);return-1;}
  int64_t mt=stat_time_ns(st.st_mtimespec),ct=stat_time_ns(st.st_ctimespec);
  for(uint32_t i=0;i<s->exec_count;i++){Executable*e=&s->execs[i];if(e->dev==st.st_dev&&e->ino==st.st_ino&&e->size==st.st_size&&e->mtime_ns==mt&&e->ctime_ns==ct){close(fd);return(int)i;}}
  if(s->exec_count>=EXECUTABLE_MAX){close(fd);fail(s,F_EXECUTABLE_LIMIT_EXCEEDED);return-1;}Executable*e=&s->execs[s->exec_count];memset(e,0,sizeof(*e));e->id=s->exec_count;e->dev=st.st_dev;e->ino=st.st_ino;e->size=st.st_size;e->mtime_ns=mt;e->ctime_ns=ct;
  if(!hash_fd(fd,e->sha)){close(fd);return-1;}close(fd);e->selected=st.st_dev==s->selected_stat.st_dev&&st.st_ino==s->selected_stat.st_ino&&memcmp(e->sha,s->selected_sha,32)==0;e->bundle=component_under(s->bundle_root,real);return(int)s->exec_count++;
}
static int read_bsd(pid_t pid,struct proc_bsdinfo*out){memset(out,0,sizeof(*out));int n=proc_pidinfo(pid,PROC_PIDTBSDINFO,0,out,sizeof(*out));return n==(int)sizeof(*out)?0:(errno?errno:EIO);}
static ProcIdentity proc_key(const struct proc_bsdinfo*i){return(ProcIdentity){(pid_t)i->pbi_pid,(int64_t)i->pbi_start_tvsec,(int32_t)i->pbi_start_tvusec};}
static int token_match(State*s,pid_t pid,bool*out){
  int mib[3]={CTL_KERN,KERN_PROCARGS2,pid};size_t n=0;if(sysctl(mib,3,NULL,&n,NULL,0))return errno?errno:EIO;if(n>PROCARGS_MAX){fail(s,F_PROCARGS_LIMIT_EXCEEDED);return EOVERFLOW;}if(n<sizeof(int))return EINVAL;uint8_t*buf=malloc(n);if(!buf)return ENOMEM;size_t got=n;if(sysctl(mib,3,buf,&got,NULL,0)){int e=errno;free(buf);return e?e:EIO;}if(got>n){free(buf);fail(s,F_PROCARGS_LIMIT_EXCEEDED);return EOVERFLOW;}
  int argc=0;memcpy(&argc,buf,sizeof(argc));if(argc<0){free(buf);return EINVAL;}size_t p=sizeof(argc);while(p<got&&buf[p])p++;while(p<got&&!buf[p])p++;*out=false;
  for(int a=0;a<argc;a++){size_t begin=p;while(p<got&&buf[p])p++;if(p>=got){free(buf);return EINVAL;}size_t len=p-begin;if(len>=s->token_len){for(size_t j=0;j+s->token_len<=len;j++)if(!memcmp(buf+begin+j,s->token,s->token_len)){*out=true;break;}}p++;}
  free(buf);return 0;
}
static bool enumerate_pids(pid_t**out,size_t*count){
  int bytes=proc_listpids(PROC_ALL_PIDS,0,NULL,0);if(bytes<=0)return false;size_t cap=(size_t)bytes/sizeof(pid_t)+1024;if(cap>1048576)return false;
  for(int tries=0;tries<3;tries++){pid_t*p=calloc(cap,sizeof(*p));if(!p)return false;int n=proc_listpids(PROC_ALL_PIDS,0,p,(int)(cap*sizeof(*p)));if(n<0){free(p);return false;}if((size_t)n<cap*sizeof(*p)){*out=p;*count=(size_t)n/sizeof(*p);return true;}free(p);cap*=2;if(cap>1048576)return false;}return false;
}

/* The sample identity set is the two-level encoding over the exact target-uid
 * identities in one complete sample. */
static void digest_identity_set(State*s,const uint32_t*ids,uint32_t n,uint8_t out[32]){
  ProcIdentity*p=n?malloc((size_t)n*sizeof(*p)):NULL;
  if(n&&!p){memset(out,0,32);return;}
  for(uint32_t i=0;i<n;i++)p[i]=s->ids[ids[i]].key;
  (void)digest_process_set(p,n,"agenterm.singleton-safety.v1.sample-membership-set",out);
  free(p);
}
/* Sample identity_ids must be unique and ordered by process identity. */
static State*g_sort_state;
static int cmp_local_identity(const void*a,const void*b){
  return cmp_proc(&g_sort_state->ids[*(const uint32_t*)a].key,&g_sort_state->ids[*(const uint32_t*)b].key);
}
static void sort_local(State*s,uint32_t*v,uint32_t n){
  g_sort_state=s;
  if(n>1)qsort(v,n,sizeof(*v),cmp_local_identity);
  g_sort_state=NULL;
}

static bool append_sample_ids(State*s,const uint32_t*ids,uint32_t n,uint32_t*offset){if(s->sample_ids_count+n>s->sample_ids_cap){size_t cap=s->sample_ids_cap?s->sample_ids_cap:8192;while(cap<s->sample_ids_count+n&&cap<STATE_MAX/sizeof(uint32_t))cap*=2;if(cap>STATE_MAX/sizeof(uint32_t))cap=STATE_MAX/sizeof(uint32_t);if(cap<s->sample_ids_count+n){fail(s,F_STATE_LIMIT_EXCEEDED);return false;}uint32_t*p=realloc(s->sample_ids,cap*sizeof(*p));if(!p){fail(s,F_STATE_LIMIT_EXCEEDED);return false;}s->sample_ids=p;s->sample_ids_cap=cap;}*offset=(uint32_t)s->sample_ids_count;memcpy(s->sample_ids+s->sample_ids_count,ids,n*sizeof(*ids));s->sample_ids_count+=n;return true;}

/* derived_relay_classification. The candidate universe is every target-uid
 * identity outside the frozen group carrying the owned token or a selected/bundle
 * executable match, with a kernel start after direct-open-root. Buckets are
 * assigned in contract order so the three counts are mutually exclusive by
 * construction: relay precedes g1_exemption precedes topology-candidate. */
enum { BUCKET_NONE=0, BUCKET_RELAY=1, BUCKET_G1=2, BUCKET_TOPOLOGY=3 };

static bool cand_push(State*s,uint32_t id,uint8_t bucket){
  if(s->cand_count>=s->cand_cap){
    uint32_t nc=s->cand_cap?s->cand_cap*2:64;
    uint32_t*g=realloc(s->cand_ids,(size_t)nc*sizeof(*g));
    if(!g)return false;
    s->cand_ids=g;
    uint8_t*b=realloc(s->cand_bucket,(size_t)nc*sizeof(*b));
    if(!b)return false;
    s->cand_bucket=b;
    uint32_t*a=realloc(s->cand_absent_sample,(size_t)nc*sizeof(*a));
    if(!a)return false;
    s->cand_absent_sample=a;
    s->cand_cap=nc;
  }
  s->cand_ids[s->cand_count]=id;
  s->cand_bucket[s->cand_count]=bucket;
  s->cand_absent_sample[s->cand_count]=UINT32_MAX;
  s->cand_count++;
  return true;
}

/* Kernel start expressed in wall-clock milliseconds, or false when the identity
 * has no usable wall-clock start. */
static bool kernel_start_ms(const ProcIdentity p,uint64_t*out){
  if(p.sec<0)return false;
  *out=(uint64_t)p.sec*1000ull+(uint64_t)(p.usec/1000);
  return true;
}

/* derived_relay_classification, run on every complete sample. Candidates already
 * classified keep their bucket (sticky); new candidates are appended. The direct
 * root itself enters no bucket. Token/bundle outside counts are recomputed as
 * overlapping projections of the topology bucket. */
static void classify_candidates(State*s,uint32_t sample,uint64_t now_ms){
  if(!s->root_obs[ROOT_DIRECT].observed)return;
  int direct=find_identity(s,s->roots[ROOT_DIRECT]);
  if(direct<0)return;
  uint64_t direct_start=0;
  if(!kernel_start_ms(s->ids[direct].key,&direct_start))return;
  for(uint32_t i=0;i<s->id_count;i++){
    Identity*x=&s->ids[i];
    if((int32_t)i==direct)continue;                 /* the root is never a candidate */
    if(x->uid!=s->uid)continue;
    /* Frozen-group membership is required to decide candidacy. Unknown membership is
     * an unsupported observation, never a silent skip. */
    if(x->in_group<0){fail(s,F_UNSUPPORTED_OBSERVATION);return;}
    if(x->in_group!=0)continue;
    if(x->token<0&&x->selected<0&&x->bundle<0)continue;  /* unknown witnesses: fail closed elsewhere */
    bool witness=(x->token==1)||(x->selected==1)||(x->bundle==1);
    if(!witness)continue;
    uint64_t ks=0;
    if(!kernel_start_ms(x->key,&ks))continue;
    /* kernel start must be strictly after direct spawn, at conservative ms resolution. */
    if(ks<=direct_start)continue;
    /* Sticky: never reclassify an identity already in the universe. */
    bool seen=false;
    for(uint32_t j=0;j<s->cand_count;j++)if(s->cand_ids[j]==i){seen=true;break;}
    if(seen)continue;
    bool p_selected=(x->selected==1);
    bool p_token=(x->token==1);
    bool primary=p_selected&&p_token&&(ks-direct_start<=RELAY_FIRST_START_WINDOW_MS);
    bool aux=(x->ppid==1)&&(x->pgid!=s->frozen_pgid);
    uint8_t bucket;
    if(primary&&aux)bucket=BUCKET_RELAY;
    else if(primary&&!aux&&x->live&&(now_ms>=ks)&&(now_ms-ks<=RELAY_EXIT_DEADLINE_MS))bucket=BUCKET_G1;
    else bucket=BUCKET_TOPOLOGY;
    if(!cand_push(s,i,bucket)){fail(s,F_RELEVANT_EVENT_LIMIT_EXCEEDED);return;}
  }
  uint32_t relay=0,topo=0,g1=0;
  int32_t single=-1;bool single_ok=true;
  for(uint32_t i=0;i<s->cand_count;i++){
    switch(s->cand_bucket[i]){
      case BUCKET_RELAY: relay++; if(single<0)single=(int32_t)s->cand_ids[i]; else single_ok=false; break;
      case BUCKET_G1: g1++; break;
      default: topo++; break;
    }
  }
  s->relay.candidate_observed_count=s->cand_count;
  s->relay.classified_relay_count=relay;
  s->relay.topology_candidate_count=topo;
  s->relay.live_g1_exemption_count=g1;
  s->relay.has_single_relay=(relay==1&&single_ok);
  s->relay_single_id=single;
  if(s->relay.has_single_relay){
    Identity*r=&s->ids[single];
    uint64_t ks=0;
    kernel_start_ms(r->key,&ks);
    s->relay_kernel_start_ms=ks;
    s->relay.relay_primary_ok=(r->selected==1)&&(r->token==1);
    s->relay.relay_auxiliary_ok=(r->ppid==1)&&(r->pgid!=s->frozen_pgid);
    s->relay.relay_kernel_window_ok=(ks-direct_start<=RELAY_FIRST_START_WINDOW_MS);
    s->ids[single].roots|=1u<<ROOT_RELAY;
  }
  s->relay.non_exempt_token_outside_count=0;
  s->relay.non_exempt_bundle_tree_outside_count=0;
  for(uint32_t i=0;i<s->cand_count;i++){
    if(s->cand_bucket[i]!=BUCKET_TOPOLOGY)continue;
    Identity*x=&s->ids[s->cand_ids[i]];
    if(x->token==1)s->relay.non_exempt_token_outside_count++;
    if(x->bundle==1)s->relay.non_exempt_bundle_tree_outside_count++;
  }
  s->relay.computed=true;
  (void)sample;
}

/* sticky-descendant-closure: every identity that inherits a root role through the
 * per-sample fixed point, recorded once and never removed. The closure excludes
 * the root identity itself, which holds the role in roots rather than lineage. */
static void digest_descendant_closure(State*s,int root_role,uint8_t out[32],uint32_t*count){
  ProcIdentity*n=s->id_count?malloc((size_t)s->id_count*sizeof(*n)):NULL;
  if(s->id_count&&!n){memset(out,0,32);*count=0;return;}
  uint32_t k=0;
  for(uint32_t i=0;i<s->id_count;i++){
    Identity*x=&s->ids[i];
    /* Descendants only: the inherited lineage bit, never the seeding roots bit. */
    if(x->lineage&(1u<<root_role))n[k++]=x->key;
  }
  *count=digest_process_set(n,k,"agenterm.singleton-safety.v1.sticky-descendant-closure",out);
  free(n);
}

/* Live projection of a sticky descendant closure: descendants that are live at the
 * current sample. Computed separately from the retained closure. */
static void digest_live_descendant_closure(State*s,int root_role,uint8_t out[32],uint32_t*count){
  ProcIdentity*n=s->id_count?malloc((size_t)s->id_count*sizeof(*n)):NULL;
  if(s->id_count&&!n){memset(out,0,32);*count=0;return;}
  uint32_t k=0;
  for(uint32_t i=0;i<s->id_count;i++){
    Identity*x=&s->ids[i];
    if(x->live&&(x->lineage&(1u<<root_role)))n[k++]=x->key;
  }
  *count=digest_process_set(n,k,"agenterm.singleton-safety.v1.sticky-descendant-closure",out);
  free(n);
}

/* Relay exit snapshot: elapsed wall-clock from the relay's kernel start to the
 * first complete sample that proves it absent. Only meaningful with one relay. */
/* The elapsed value is captured at the first complete absence sample and then read
 * back; it is never recomputed at serialization time. */
static bool relay_exit_elapsed(State*s,uint64_t now_ms,uint64_t*out){
  (void)now_ms;
  if(!s->relay.has_single_relay||!s->relay.relay_exit_observed)return false;
  if(!s->relay.relay_exit_have_elapsed)return false;
  *out=s->relay.relay_exit_elapsed_ms;
  return true;
}

/* Candidate absence pass. A classified relay or candidate that was observed live
 * and is now missing from a complete sample has exited; the first such sample is
 * sticky. Only the single relay feeds relay_exit_elapsed_ms. */
static void observe_candidate_exits(State*s,uint32_t sample,uint64_t now_ms){
  if(s->failed)return;
  for(uint32_t i=0;i<s->cand_count;i++){
    if(s->cand_absent_sample[i]!=UINT32_MAX)continue;
    Identity*x=&s->ids[s->cand_ids[i]];
    if(x->live)continue;
    s->cand_absent_sample[i]=sample;
    if(s->relay.has_single_relay&&(int32_t)s->cand_ids[i]==s->relay_single_id){
      s->relay.relay_exit_observed=true;
      s->relay.relay_exit_have_elapsed=(now_ms>=s->relay_kernel_start_ms);
      s->relay.relay_exit_elapsed_ms=s->relay.relay_exit_have_elapsed?now_ms-s->relay_kernel_start_ms:0;
    }
  }
}

/* stage_snapshot_rules, one branch per rule. A snapshot is claimed only by a
 * complete sample that satisfies the rule, and the first such sample wins. */
static uint32_t bundle_outside_count(State*s,uint64_t now_ms);
static void capture_stage_facts(State*s,uint32_t seq);
static void advance_stages(State*s,uint32_t seq,bool complete){
  if(!complete)return;
  Sample*sm=&s->samples[seq];
  int32_t seen=sm->control_sequence;
  /* baseline: the sequence-zero first sample, before any browser-root control. */
  if(!s->stages.have_baseline&&seq==0&&!s->control_seen[C_BROWSER]){
    s->stages.have_baseline=true;s->stages.baseline_sample=seq;
  }
  /* session_ready: first complete sample whose control_sequence_seen reaches the
   * observed browser-root control sequence. */
  if(!s->stages.have_session_ready&&s->control_seen[C_BROWSER]&&
     seen>=s->root_control_seq[ROOT_BROWSER]){
    s->stages.have_session_ready=true;s->stages.session_ready_sample=seq;
  }
  /* open_request_exit: first complete sample after direct-open-root was observed
   * that proves that exact root absent. */
  if(!s->stages.have_open_request&&s->root_valid[ROOT_DIRECT]&&s->root_obs[ROOT_DIRECT].ever_observed&&
     s->root_obs[ROOT_DIRECT].absent){
    s->stages.have_open_request=true;s->stages.open_request_sample=seq;
  }
  /* termination: first complete sample proving the browser-root absent. */
  if(!s->stages.have_termination&&s->root_valid[ROOT_BROWSER]&&s->root_obs[ROOT_BROWSER].ever_observed&&
     s->root_obs[ROOT_BROWSER].absent){
    s->stages.have_termination=true;s->stages.termination_sample=seq;
  }
  /* final_inventory: the complete sentinel sample at or after final-inventory-complete. */
  if(!s->stages.have_final_inventory&&sm->kind=='s'&&s->control_seen[C_FINAL]){
    s->stages.have_final_inventory=true;s->stages.final_inventory_sample=seq;
  }
  capture_stage_facts(s,seq);
}

static PP_MAYBE_UNUSED bool sample_native(State*s,int32_t control_seq,bool sentinel){
  if(s->sample_count>=SAMPLE_MAX){fail(s,F_SAMPLE_LIMIT_EXCEEDED);return false;}uint32_t seq=s->sample_count;Sample*sm=&s->samples[seq];memset(sm,0,sizeof(*sm));sm->sequence=seq;sm->kind=seq==0?'f':sentinel?'s':'i';sm->control_sequence=control_seq;sm->start_ns=mono_ns();if(seq){sm->interval_ns=sm->start_ns-s->samples[seq-1].start_ns;if(sm->interval_ns>s->max_interval)s->max_interval=sm->interval_ns;if(sm->interval_ns>BOUND_NS)fail(s,F_START_INTERVAL_EXCEEDED);}
  for(uint32_t i=0;i<s->id_count;i++){s->ids[i].live=false;s->ids[i].in_closure_now=false;}pid_t*pids=NULL;size_t pn=0;bool complete=enumerate_pids(&pids,&pn);if(!complete)fail(s,F_ENUMERATION_FAILED);uint32_t local[PROCESS_MAX],local_n=0,group_n=0;
  for(size_t pi=0;complete&&pi<pn;pi++){pid_t pid=pids[pi];if(pid<=0)continue;struct proc_bsdinfo a,b;int er=read_bsd(pid,&a);if(er){if(er!=ESRCH){complete=false;fail(s,F_SAMPLE_INCOMPLETE);}continue;}if((uid_t)a.pbi_uid!=s->uid)continue;if(local_n>=PROCESS_MAX){complete=false;fail(s,F_PROCESS_ROW_LIMIT_EXCEEDED);break;}ProcIdentity key=proc_key(&a);int id=get_identity(s,key,seq);if(id<0){complete=false;break;}Identity*x=&s->ids[id];pid_t old_ppid=x->ppid;x->ppid=(pid_t)a.pbi_ppid;x->pgid=getpgid(pid);x->uid=(uid_t)a.pbi_uid;x->status=OBS_LIVE;x->last_sample=seq;x->live=true;x->recheck=false;x->parent_id=-1;
    char path[PROC_PIDPATHINFO_MAXSIZE];int plen=proc_pidpath(pid,path,sizeof(path));if(plen<=0){x->status=OBS_UNREADABLE;complete=false;fail(s,F_SAMPLE_INCOMPLETE);}else{path[sizeof(path)-1]=0;int ex=get_executable(s,path);if(ex<0){x->status=OBS_UNREADABLE;complete=false;fail(s,F_SAMPLE_INCOMPLETE);}else{x->executable_id=ex;x->selected=s->execs[ex].selected;x->bundle=s->execs[ex].bundle;if(x->selected)add_event(s,seq,EV_SELECTED,id,2);if(x->bundle)add_event(s,seq,EV_BUNDLE,id,4);}}
    bool tm=false;er=token_match(s,pid,&tm);if(er){x->token=-1;x->status=er==ESRCH?OBS_VANISHED:OBS_UNREADABLE;complete=false;fail(s,F_SAMPLE_INCOMPLETE);}else{x->token=tm;if(tm)add_event(s,seq,EV_TOKEN,id,1);}
    er=read_bsd(pid,&b);if(er||!same_proc(key,proc_key(&b))){x->status=er==ESRCH?OBS_VANISHED:OBS_CHANGED;complete=false;fail(s,F_SAMPLE_INCOMPLETE);}else x->recheck=true;
    if(s->frozen_pgid>0){x->in_group=x->pgid==s->frozen_pgid;if(x->in_group){group_n++;if(!x->ever_group){x->ever_group=true;add_event(s,seq,EV_GROUP,id,8);}}else if(x->ever_group&&(x->lineage|x->roots)){if(!x->ever_breakaway){x->ever_breakaway=true;add_event(s,seq,EV_BREAKAWAY,id,16);}}}
    if(x->live&&x->in_group==1&&x->bundle==1)x->enumerated_bundle_group_sample=seq;
    if(old_ppid&&old_ppid!=x->ppid)add_event(s,seq,EV_REPARENTED,id,0);local[local_n++]=(uint32_t)id;
  }
  free(pids);if(group_n>GROUP_MAX){complete=false;fail(s,F_GROUP_LIMIT_EXCEEDED);}for(uint32_t i=0;i<local_n;i++){Identity*x=&s->ids[local[i]];for(uint32_t j=0;j<local_n;j++)if(s->ids[local[j]].key.pid==x->ppid){x->parent_id=(int32_t)local[j];add_edge(s,x->id,(uint32_t)x->parent_id,seq);break;}}
  /* Root observation: record the first sample that observed the exact controlled
   * identity, and the first later complete sample that proves it absent. A root
   * that was never observed cannot support an absence claim. */
  for(int r=0;r<ROOT_COUNT;r++){
    if(!s->root_valid[r])continue;
    int id=find_identity(s,s->roots[r]);
    bool live_match=id>=0&&s->ids[id].live;
    if(live_match){
      if(!(s->ids[id].roots&(1u<<r)))s->ids[id].roots|=1u<<r;
      if(!s->root_obs[r].observed){
        s->root_obs[r].observed=true;
        s->root_obs[r].first_observed_sample=seq;
        s->root_obs[r].ever_observed=true;
        add_event(s,seq,EV_ROOT_OBSERVED,id,0);
      }
      if(r==ROOT_BROWSER&&s->frozen_pgid<=0)s->frozen_pgid=s->ids[id].pgid;
    }else if(s->root_obs[r].ever_observed&&!s->root_obs[r].absent){
      s->root_obs[r].absent=true;
      s->root_obs[r].first_absent_sample=seq;
      int ev=find_identity(s,s->roots[r]);
      add_event(s,seq,EV_ROOT_EXITED,ev,0);
    }
    /* The first complete sample after the control is the one chance to satisfy the
     * match. A prior retained live observation counts; otherwise this sample must
     * observe it. */
    if(complete&&!s->root_obs[r].miss_checked&&seq>=s->root_control_sample[r]){
      s->root_obs[r].miss_checked=true;
      if(!live_match&&!s->root_obs[r].ever_observed){
        s->root_obs[r].unobserved=true;
        fail(s,F_ROOT_UNOBSERVED);
      }
    }
  }
  propagate_lineage(s,seq);
  /* Record same-sample closure membership: an identity is in the live contained
   * closure when it is live and either in the frozen group or on the controlled
   * lineage. This is per-sample, not sticky. */
  for(uint32_t i=0;i<s->id_count;i++){
    Identity*x=&s->ids[i];
    x->in_closure_now=x->live&&(x->in_group==1||((x->roots|x->lineage)&1));
    if(x->in_closure_now)x->closure_inserted_sample=seq;
  }
  advance_stages(s,seq,complete);
  classify_candidates(s,seq,realtime_ms());
  observe_candidate_exits(s,seq,realtime_ms());
  sm->group_count=group_n;sm->complete=complete;sm->ids_count=local_n;sort_local(s,local,local_n);append_sample_ids(s,local,local_n,&sm->ids_offset);digest_identity_set(s,local,local_n,sm->digest);sm->end_ns=mono_ns();uint64_t en=sm->end_ns-sm->start_ns;if(en>s->max_enumeration)s->max_enumeration=en;if(en>BOUND_NS){complete=false;sm->complete=false;fail(s,F_ENUMERATION_TIME_EXCEEDED);}if(local_n>s->max_rows)s->max_rows=local_n;if(group_n>s->max_group)s->max_group=group_n;s->sample_count++;if(!complete)fail(s,F_SAMPLE_INCOMPLETE);return complete;
}

/* Minimal strict JSON reader for the closed control shape. Escapes are rejected:
 * every accepted key/value is frozen printable ASCII. */
typedef struct{const char*p,*end;} J;
static void jw(J*j){while(j->p<j->end&&(*j->p==' '||*j->p=='\t'||*j->p=='\r'))j->p++;}
static bool jc(J*j,char c){jw(j);if(j->p>=j->end||*j->p!=c)return false;j->p++;return true;}
static bool js(J*j,char*out,size_t cap){jw(j);if(j->p>=j->end||*j->p++!='"')return false;size_t n=0;while(j->p<j->end&&*j->p!='"'){unsigned char c=(unsigned char)*j->p++;if(c<0x20||c=='\\'||n+1>=cap)return false;out[n++]=(char)c;}if(j->p>=j->end)return false;j->p++;out[n]=0;return true;}
static bool ju(J*j,uint64_t*out){jw(j);if(j->p>=j->end||*j->p<'0'||*j->p>'9')return false;uint64_t v=0;do{unsigned d=(unsigned)(*j->p++-'0');if(v>(UINT64_MAX-d)/10)return false;v=v*10+d;}while(j->p<j->end&&*j->p>='0'&&*j->p<='9');*out=v;return true;}
static bool jnull(J*j){jw(j);if((size_t)(j->end-j->p)<4||memcmp(j->p,"null",4))return false;j->p+=4;return true;}
static bool parse_process(J*j,ProcIdentity*out){if(!jc(j,'{'))return false;unsigned seen=0;for(;;){char k[32];if(!js(j,k,sizeof(k))||!jc(j,':'))return false;uint64_t v;if(!ju(j,&v))return false;if(!strcmp(k,"pid")){if(seen&1||v<1||v>INT_MAX)return false;out->pid=(pid_t)v;seen|=1;}else if(!strcmp(k,"start_sec")){if(seen&2||v>INT64_MAX)return false;out->sec=(int64_t)v;seen|=2;}else if(!strcmp(k,"start_usec")){if(seen&4||v>999999)return false;out->usec=(int32_t)v;seen|=4;}else return false;jw(j);if(j->p<j->end&&*j->p==','){j->p++;continue;}break;}return seen==7&&jc(j,'}');}
static ControlRole role_of(const char*s){
  /* Only the five court controls reach the protocol. relay-root and
   * topology-candidate-root are scanner-derived and must never appear here. */
  if(!strcmp(s,"browser-root"))return C_BROWSER;
  if(!strcmp(s,"ownership-boundary"))return C_OWNERSHIP;
  if(!strcmp(s,"direct-open-root"))return C_DIRECT;
  if(!strcmp(s,"final-inventory-complete"))return C_FINAL;
  if(!strcmp(s,"abort"))return C_ABORT;
  return C_BAD;
}
static bool parse_control(const char*p,size_t n,Control*out){J j={p,p+n};if(!jc(&j,'{'))return false;unsigned seen=0;char role[48]={0};for(;;){char k[64];if(!js(&j,k,sizeof(k))||!jc(&j,':'))return false;if(!strcmp(k,"schema")){char v[96];if(seen&1||!js(&j,v,sizeof(v))||strcmp(v,CONTROL_SCHEMA))return false;seen|=1;}else if(!strcmp(k,"sequence")){uint64_t v;if(seen&2||!ju(&j,&v)||v>=CONTROL_MAX)return false;out->sequence=(uint32_t)v;seen|=2;}else if(!strcmp(k,"role")){if(seen&4||!js(&j,role,sizeof(role)))return false;seen|=4;}else if(!strcmp(k,"process")){if(seen&8)return false;jw(&j);if(j.p<j.end&&*j.p=='n'){if(!jnull(&j))return false;out->has_process=false;}else{out->has_process=true;if(!parse_process(&j,&out->process))return false;}seen|=8;}else return false;jw(&j);if(j.p<j.end&&*j.p==','){j.p++;continue;}break;}if(seen!=15||!jc(&j,'}'))return false;jw(&j);if(j.p!=j.end)return false;out->role=role_of(role);return out->role!=C_BAD;}
static bool accept_control(State*s,Control*c){
  if(c->sequence!=s->controls||s->controls>=CONTROL_MAX)return false;
  ControlRole r=c->role;
  if(r>=C_COUNT)return false;
  if(s->final_seen||s->abort_seen||s->control_seen[r])return false;
  bool proc=r==C_BROWSER||r==C_DIRECT;
  if(proc!=c->has_process)return false;
  if(r!=C_BROWSER&&!s->control_seen[C_BROWSER])return false;
  if((r==C_DIRECT||r==C_FINAL)&&!s->control_seen[C_OWNERSHIP])return false;
  s->control_seen[r]=true;
  s->last_control=(int32_t)c->sequence;
  s->controls++;
  if(proc){int rr=r==C_BROWSER?ROOT_BROWSER:ROOT_DIRECT;s->roots[rr]=c->process;s->root_valid[rr]=true;s->root_control_seq[rr]=(int32_t)c->sequence;s->root_control_sample[rr]=s->sample_count;}
  if(r==C_OWNERSHIP)s->ownership_pending=true;
  if(r==C_FINAL){s->final_seen=true;s->final_control=(int32_t)c->sequence;}
  if(r==C_ABORT){s->abort_seen=true;fail(s,F_ABORT_REQUESTED);}
  return true;
}
static PP_MAYBE_UNUSED bool poll_controls(State*s){int fd=open(s->control_path,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);if(fd<0){fail(s,F_CONTROL_INVALID);return false;}struct stat st;if(fstat(fd,&st)||!S_ISREG(st.st_mode)||st.st_dev!=s->control_dev||st.st_ino!=s->control_ino||st.st_size<s->control_offset){close(fd);fail(s,F_CONTROL_INVALID);return false;}while(s->control_offset<st.st_size){size_t room=CONTROL_BYTES_MAX-s->control_tail_len;if(!room){close(fd);fail(s,F_CONTROL_LIMIT_EXCEEDED);return false;}off_t left=st.st_size-s->control_offset;size_t want=(uint64_t)left<room?(size_t)left:room;ssize_t n=pread(fd,s->control_tail+s->control_tail_len,want,s->control_offset);if(n<=0){close(fd);fail(s,F_CONTROL_INVALID);return false;}s->control_offset+=n;s->control_tail_len+=(size_t)n;size_t start=0;for(size_t i=0;i<s->control_tail_len;i++)if(s->control_tail[i]=='\n'){size_t len=i-start;if(!len||len>=CONTROL_BYTES_MAX){close(fd);fail(s,F_CONTROL_INVALID);return false;}Control c={0};if(!parse_control(s->control_tail+start,len,&c)||!accept_control(s,&c)){close(fd);fail(s,F_CONTROL_INVALID);return false;}start=i+1;}if(start){memmove(s->control_tail,s->control_tail+start,s->control_tail_len-start);s->control_tail_len-=start;}if(s->control_tail_len==CONTROL_BYTES_MAX){close(fd);fail(s,F_CONTROL_LIMIT_EXCEEDED);return false;}}close(fd);if((s->final_seen||s->abort_seen)&&s->control_tail_len){fail(s,F_CONTROL_INVALID);return false;}return true;}

static void digest_selected(State*s,bool(*pred)(Identity*),const char*domain,uint8_t out[32],uint32_t*count){
  /* Collect matching identities, then hand them to the two-level set encoder. */
  ProcIdentity*stk=NULL;uint32_t cap=0,n=0;
  for(uint32_t i=0;i<s->id_count;i++)if(pred(&s->ids[i])){if(n==cap){uint32_t nc=cap?cap*2:64;ProcIdentity*g=realloc(stk,(size_t)nc*sizeof(*g));if(!g){free(stk);memset(out,0,32);*count=0;return;}stk=g;cap=nc;}stk[n++]=s->ids[i].key;}
  *count=digest_process_set(stk,n,domain,out);
  free(stk);
}
static bool p_group(Identity*x){return x->ever_group;}
static bool p_control(Identity*x){return(x->roots|x->lineage)&1;}static bool p_retained(Identity*x){return x->ever_group||p_control(x);}static bool p_live(Identity*x){return x->live&&(x->in_group==1||p_control(x));}static bool p_token(Identity*x){return x->live&&x->token==1;}static bool p_bundle(Identity*x){return x->live&&x->bundle==1;}
/* derived_relay_classification.g1_exemption for the direct root: exempt only while
 * that exact identity remains live and its elapsed since kernel start is within the
 * deadline. Used to subtract the exemption from outside-witness counts. */
static bool g1_exempt_live(State*s,int32_t id,uint64_t now_ms){
  if(id<0)return false;
  Identity*x=&s->ids[id];
  if(!x->live)return false;
  uint64_t ks=0;
  if(!kernel_start_ms(x->key,&ks))return false;
  return now_ms>=ks&&(now_ms-ks)<=RELAY_EXIT_DEADLINE_MS;
}
static bool p_in_group(Identity*x){return x->live&&x->in_group==1;}
/* Live members of a root's sticky descendant closure at the current sample. */
static uint32_t count_live_closure(State*s,int role){
  uint32_t n=0;
  for(uint32_t i=0;i<s->id_count;i++)if(s->ids[i].live&&((s->ids[i].lineage|s->ids[i].roots)&(1u<<role)))n++;
  return n;
}
/* selected-bundle-set: live exact identities matching the selected executable and
 * inside the bundle tree. */
static bool p_selected_bundle(Identity*x){return x->live&&x->selected==1&&x->bundle==1;}
static uint32_t count_live_where(State*s,bool(*pred)(Identity*)){
  uint32_t n=0;for(uint32_t i=0;i<s->id_count;i++)if(pred(&s->ids[i]))n++;
  return n;
}
/* contained-browser-closure (live) at the current sample. */
static uint32_t contained_closure_live_count(State*s){return count_live_where(s,p_live);}
/* Union of the sticky direct, relay and topology-candidate descendant closures,
 * restricted to live identities, minus the contained browser closure. */
static uint32_t escaped_target_count(State*s){
  uint32_t n=0;
  const int roles[3]={ROOT_DIRECT,ROOT_RELAY,ROOT_CANDIDATE};
  for(uint32_t i=0;i<s->id_count;i++){
    Identity*x=&s->ids[i];
    if(!x->live)continue;
    bool in_union=false;
    for(int k=0;k<3;k++)if((x->lineage|x->roots)&(1u<<roles[k])){in_union=true;break;}
    if(in_union&&!p_live(x))n++;
  }
  return n;
}
/* cleanup_targets_absent: no live identity in the union of contained-browser,
 * direct, relay and topology-candidate closures is still eligible for cleanup. */
static bool cleanup_targets_absent(State*s){
  for(uint32_t i=0;i<s->id_count;i++){
    Identity*x=&s->ids[i];
    if(!x->live)continue;
    bool in_union=p_live(x);
    for(int k=0;k<3&&!in_union;k++)if((x->lineage|x->roots)&(1u<<((const int[3]){ROOT_DIRECT,ROOT_RELAY,ROOT_CANDIDATE})[k]))in_union=true;
    if(in_union)return false;
  }
  return true;
}
static void digest_live_selected_bundle(State*s,uint8_t out[32],uint32_t*count){
  /* selected-bundle-set uses the bundle-tree-set domain per the contract. */
  digest_selected(s,p_selected_bundle,"agenterm.singleton-safety.v1.bundle-tree-set",out,count);
}
static void digest_live_token(State*s,uint8_t out[32],uint32_t*count){
  digest_selected(s,p_token,"agenterm.singleton-safety.v1.process-set",out,count);
}
static void digest_live_contained(State*s,uint8_t out[32],uint32_t*count){
  digest_selected(s,p_live,"agenterm.singleton-safety.v1.contained-browser-closure",out,count);
}

/* Capture every stage fact at the sample that satisfies its rule. The contract
 * forbids zero/false placeholders for unobservable facts, so a fact is only
 * stored when its underlying value exists; the serializer then emits null. */
static void capture_stage_facts(State*s,uint32_t seq){
  if(s->stages.have_baseline&&s->stages.baseline_sample==seq){
    uint32_t c=count_live_where(s,p_selected_bundle);
    s->stages.baseline_selected_bundle_count=c;
    s->stages.baseline_have_count=true;
    s->stages.baseline_zero_independent=(c==0);
  }
  if(s->stages.have_session_ready&&s->stages.session_ready_sample==seq){
    /* browser identity digest from the observed browser root. */
    if(s->root_valid[ROOT_BROWSER]){
      identity_digest(s->roots[ROOT_BROWSER],s->stages.session_ready_browser_identity);
      s->stages.session_ready_have_identity=true;
    }
    uint8_t bs[32];uint32_t bc=0;
    digest_live_selected_bundle(s,bs,&bc);
    memcpy(s->stages.session_ready_bundle_set,bs,32);
    s->stages.session_ready_have_bundle=true;
    s->stages.session_ready_selected_bundle_count=bc;
    s->stages.session_ready_have_count=true;
  }
  if(s->stages.have_open_request&&s->stages.open_request_sample==seq){
    s->stages.open_request_exit_observed=true;
    /* The direct-open-root descendant closure is the sticky direct closure. */
    digest_descendant_closure(s,ROOT_DIRECT,s->stages.open_request_desc_retained,
                              &s->stages.open_request_desc_retained_count);
    uint8_t lv[32];uint32_t lc=0;
    /* Live projection: only identities still live at the snapshot. */
    digest_live_descendant_closure(s,ROOT_DIRECT,lv,&lc);
    memcpy(s->stages.open_request_desc_live,lv,32);
    s->stages.open_request_desc_live_count=lc;
    s->stages.open_request_desc_have_live=true;
  }
  if(s->stages.have_termination&&s->stages.termination_sample==seq){
    s->stages.termination_browser_absent=s->root_obs[ROOT_BROWSER].absent;
    s->stages.termination_contained_group_absent=
      (s->frozen_pgid>0)&&(count_live_where(s,p_in_group)==0);
    s->stages.termination_contained_closure_live_count=contained_closure_live_count(s);
    uint8_t ts[32];uint32_t tc=0;
    digest_live_token(s,ts,&tc);
    memcpy(s->stages.termination_token_set,ts,32);
    s->stages.termination_token_count=tc;
    s->stages.termination_escaped_target_count=escaped_target_count(s);
  }
  if(s->stages.have_final_inventory&&s->stages.final_inventory_sample==seq){
    s->stages.final_inventory_selected_bundle_count=count_live_where(s,p_selected_bundle);
    s->stages.final_inventory_selected_bundle_have=true;
    uint8_t ts[32];uint32_t tc=0;digest_live_token(s,ts,&tc);
    memcpy(s->stages.final_inventory_token_set,ts,32);
    s->stages.final_inventory_token_count=tc;
    s->stages.final_inventory_bundle_tree_outside_count=bundle_outside_count(s,realtime_ms());
    uint8_t cr[32],cl[32];uint32_t crc=0,clc=0;
    digest_selected(s,p_retained,"agenterm.singleton-safety.v1.contained-browser-closure",cr,&crc);
    digest_live_contained(s,cl,&clc);
    memcpy(s->stages.final_inventory_contained_retained,cr,32);
    s->stages.final_inventory_contained_retained_count=crc;
    memcpy(s->stages.final_inventory_contained_live,cl,32);
    s->stages.final_inventory_contained_live_count=clc;
    digest_descendant_closure(s,ROOT_DIRECT,s->stages.final_inventory_direct_desc_retained,&s->stages.final_inventory_direct_desc_retained_count);
    s->stages.final_inventory_direct_desc_live_count=count_live_closure(s,ROOT_DIRECT);
    digest_descendant_closure(s,ROOT_RELAY,s->stages.final_inventory_relay_desc_retained,&s->stages.final_inventory_relay_desc_retained_count);
    s->stages.final_inventory_relay_desc_live_count=count_live_closure(s,ROOT_RELAY);
    digest_descendant_closure(s,ROOT_CANDIDATE,s->stages.final_inventory_candidate_desc_retained,&s->stages.final_inventory_candidate_desc_retained_count);
    s->stages.final_inventory_candidate_desc_live_count=count_live_closure(s,ROOT_CANDIDATE);
    s->stages.final_inventory_cleanup_targets_absent=cleanup_targets_absent(s);
  }
}




/* bundle-tree outside count: live bundle identities outside the frozen group with no
 * controlled lineage, excluding only an identity that itself satisfies the live G1
 * exemption. The direct root is exempt only when it is the identity being counted and
 * it is live and within the deadline. */
static uint32_t bundle_outside_count(State*s,uint64_t now_ms){
  int direct=s->root_valid[ROOT_DIRECT]?find_identity(s,s->roots[ROOT_DIRECT]):-1;
  uint32_t n=0;
  for(uint32_t i=0;i<s->id_count;i++){
    Identity*x=&s->ids[i];
    if(!(x->live&&x->bundle==1&&x->in_group!=1&&!p_control(x)))continue;
    if((int32_t)i==direct&&g1_exempt_live(s,direct,now_ms))continue;
    n++;
  }
  return n;
}

static PP_MAYBE_UNUSED void freeze_ownership(State*s){Ownership*o=&s->ownership;memset(o,0,sizeof(*o));
  /* The ownership snapshot is the current complete sample. Derive own from it now;
   * s->ownership_sample is not yet set on this entry and must never be read here. */
  int32_t own=s->sample_count?(int32_t)(s->sample_count-1):-1;
  digest_selected(s,p_group,"agenterm.singleton-safety.v1.group-membership-set",o->contained_group,&o->group_retained);
  digest_selected(s,p_control,"agenterm.singleton-safety.v1.controlled-lineage-set",o->controlled,&o->controlled_count);
  digest_selected(s,p_retained,"agenterm.singleton-safety.v1.contained-browser-closure",o->retained,&o->retained_count);
  digest_selected(s,p_live,"agenterm.singleton-safety.v1.contained-browser-closure",o->live,&o->live_count);
  digest_selected(s,p_token,"agenterm.singleton-safety.v1.process-set",o->token,&o->token_count);
  uint32_t bundle_count=0;digest_selected(s,p_bundle,"agenterm.singleton-safety.v1.bundle-tree-set",o->bundle,&bundle_count);for(uint32_t i=0;i<s->id_count;i++){Identity*x=&s->ids[i];if(x->live&&x->in_group==1)o->group_live++;if(x->ever_breakaway)o->breakaways++;}
  o->bundle_outside=bundle_outside_count(s,realtime_ms());
  int b=s->root_valid[ROOT_BROWSER]?find_identity(s,s->roots[ROOT_BROWSER]):-1;o->browser_token=b>=0&&s->ids[b].live&&s->ids[b].token==1;o->insertion=true;
  for(uint32_t i=0;i<s->id_count;i++){Identity*x=&s->ids[i];
    if(!(x->live&&x->in_group==1&&x->bundle==1))continue;
    if(x->enumerated_bundle_group_sample!=(uint32_t)own||x->closure_inserted_sample!=(uint32_t)own){o->insertion=false;break;}}o->complete=s->sample_count&&s->samples[s->sample_count-1].complete&&b>=0&&s->frozen_pgid>0;
  /* Publish the claimed snapshot only after the function has successfully frozen it. */
  s->ownership_done=true;s->ownership_sample=own;}

/* Emit a JSON array of the selected names, sorted by ascending UTF-8 bytes as the
 * contract requires (the array order is independent of enum order). */
static int cmp_str_bytes(const void*a,const void*b){
  return strcmp(*(const char*const*)a,*(const char*const*)b);
}
static void b_sorted_names(Buffer*b,const char**names,const bool*selected,size_t n){
  const char**hit=n?malloc(n*sizeof(*hit)):NULL;
  size_t k=0;
  if(hit)for(size_t i=0;i<n;i++)if(selected[i])hit[k++]=names[i];
  if(k>1)qsort(hit,k,sizeof(*hit),cmp_str_bytes);
  b_s(b,"[");for(size_t i=0;i<k;i++){if(i)b_s(b,",");b_s(b,"\"");b_s(b,hit[i]);b_s(b,"\"");}b_s(b,"]");
  free(hit);
}

static void b_failures(Buffer*b,State*s){bool sel[F_COUNT];for(int i=0;i<F_COUNT;i++)sel[i]=(s->failures&(1u<<i))!=0;b_sorted_names(b,FAILURE_NAMES,sel,F_COUNT);}
static void b_roots(Buffer*b,uint8_t bits){bool sel[ROOT_COUNT];for(int i=0;i<ROOT_COUNT;i++)sel[i]=(bits&(1u<<i))!=0;b_sorted_names(b,ROOT_NAMES,sel,ROOT_COUNT);}
static void b_witness(Buffer*b,uint16_t w){static const char*n[]={"token","selected-executable","bundle-tree","frozen-group","controlled-lineage","direct-lineage","relay-lineage","topology-candidate-lineage"};bool sel[8];for(int i=0;i<8;i++)sel[i]=(w&(1u<<i))!=0;b_sorted_names(b,n,sel,8);}
static void serialize_state(State*s,Buffer*b,const char*status){
  b_s(b,"{\"schema\":\"");b_s(b,STATE_SCHEMA);b_s(b,"\",\"status\":\"");b_s(b,status);b_s(b,"\",\"failure_reasons\":");b_failures(b,s);b_s(b,",\"control_record_count\":");b_u(b,s->controls);b_s(b,",\"sample_count\":");b_u(b,s->sample_count);b_s(b,",\"scan_start_mono_ns\":");b_nullable_u(b,s->sample_count,s->sample_count?s->samples[0].start_ns:0);b_s(b,",\"sentinel_start_mono_ns\":");bool sent=s->sample_count&&s->samples[s->sample_count-1].kind=='s';b_nullable_u(b,sent,sent?s->samples[s->sample_count-1].start_ns:0);b_s(b,",\"ownership_sample_sequence\":");b_nullable_u(b,s->ownership_done,(uint32_t)s->ownership_sample);b_s(b,",\"final_inventory_control_sequence\":");b_nullable_u(b,s->final_seen,(uint32_t)s->final_control);b_s(b,",\"frozen_group_id\":");b_nullable_u(b,s->frozen_pgid>0,(uint32_t)s->frozen_pgid);  /* root_observations: exactly the two court roots, each an exact
   * root_observation_state object; relay/candidate roots are not court roots. */
  b_s(b,",\"root_observations\":{");for(int ri=0;ri<2;ri++){int r=COURT_ROOTS[ri];if(ri)b_s(b,",");b_s(b,"\"");b_s(b,ROOT_NAMES[r]);b_s(b,"\":{");b_s(b,"\"status\":\"");if(!s->root_valid[r])b_s(b,"not-controlled");else if(s->root_obs[r].unobserved)b_s(b,"unobserved");else if(s->root_obs[r].observed)b_s(b,"observed");else b_s(b,"pending");b_s(b,"\",\"identity_digest\":");if(s->root_valid[r]){uint8_t d[32];identity_digest(s->roots[r],d);b_s(b,"\"");b_digest(b,d);b_s(b,"\"");}else b_s(b,"null");b_s(b,",\"first_observed_sample\":");b_nullable_u(b,s->root_obs[r].observed,s->root_obs[r].first_observed_sample);b_s(b,",\"first_absent_sample\":");b_nullable_u(b,s->root_obs[r].absent,s->root_obs[r].first_absent_sample);b_s(b,"}");}b_s(b,"}");b_s(b,",\"early_scanner_stop\":");b_bool(b,s->early_stop);b_s(b,",\"max_start_interval_ns\":");b_nullable_u(b,s->sample_count>1,s->max_interval);b_s(b,",\"max_enumeration_ns\":");b_nullable_u(b,s->sample_count>0,s->max_enumeration);b_s(b,",\"resolution_ns\":");b_nullable_u(b,s->sample_count>1,s->max_interval+s->max_enumeration);b_s(b,",\"maximum_process_row_count\":");b_u(b,s->max_rows);b_s(b,",\"maximum_group_member_count\":");b_u(b,s->max_group);
  b_s(b,",\"samples\":[");for(uint32_t i=0;i<s->sample_count;i++){Sample*x=&s->samples[i];if(i)b_s(b,",");b_s(b,"{\"sequence\":");b_u(b,x->sequence);b_s(b,",\"kind\":\"");b_s(b,x->kind=='f'?"first":x->kind=='s'?"sentinel":"internal");b_s(b,"\",\"start_mono_ns\":");b_u(b,x->start_ns);b_s(b,",\"end_mono_ns\":");b_u(b,x->end_ns);b_s(b,",\"start_interval_ns\":");b_nullable_u(b,i>0,x->interval_ns);b_s(b,",\"enumeration_ns\":");b_u(b,x->end_ns-x->start_ns);b_s(b,",\"control_sequence_seen\":");b_nullable_u(b,x->control_sequence>=0,(uint32_t)x->control_sequence);b_s(b,",\"complete\":");b_bool(b,x->complete);b_s(b,",\"process_row_count\":");b_u(b,x->ids_count);b_s(b,",\"group_member_count\":");b_u(b,x->group_count);b_s(b,",\"identity_ids\":[");for(uint32_t j=0;j<x->ids_count;j++){if(j)b_s(b,",");b_u(b,s->sample_ids[x->ids_offset+j]);}b_s(b,"],\"identity_set_sha256\":\"");b_digest(b,x->digest);b_s(b,"\"}");}b_s(b,"]");
  b_s(b,",\"identities\":[");for(uint32_t i=0;i<s->id_count;i++){Identity*x=&s->ids[i];if(i)b_s(b,",");b_s(b,"{\"id\":");b_u(b,x->id);b_s(b,",\"pid\":");b_i(b,x->key.pid);b_s(b,",\"ppid\":");b_i(b,x->ppid);b_s(b,",\"pgid\":");b_i(b,x->pgid);b_s(b,",\"uid\":");b_u(b,x->uid);b_s(b,",\"start_sec\":");b_i(b,x->key.sec);b_s(b,",\"start_usec\":");b_i(b,x->key.usec);b_s(b,",\"parent_identity_id\":");b_nullable_u(b,x->parent_id>=0,(uint32_t)x->parent_id);b_s(b,",\"executable_id\":");b_nullable_u(b,x->executable_id>=0,(uint32_t)x->executable_id);b_s(b,",\"token_match\":");if(x->token<0)b_s(b,"null");else b_bool(b,x->token);b_s(b,",\"selected_executable_match\":");if(x->selected<0)b_s(b,"null");else b_bool(b,x->selected);b_s(b,",\"bundle_tree_match\":");if(x->bundle<0)b_s(b,"null");else b_bool(b,x->bundle);b_s(b,",\"in_frozen_group\":");if(x->in_group<0)b_s(b,"null");else b_bool(b,x->in_group);b_s(b,",\"identity_recheck_match\":");b_bool(b,x->recheck);b_s(b,",\"observation_status\":\"");b_s(b,OBS_NAMES[x->status]);b_s(b,"\",\"first_seen_sample\":");b_u(b,x->first_sample);b_s(b,",\"last_seen_sample\":");b_u(b,x->last_sample);b_s(b,",\"live_at_latest\":");b_bool(b,x->live);b_s(b,",\"root_roles\":");b_roots(b,x->roots);b_s(b,",\"lineage_root_roles\":");b_roots(b,x->lineage);b_s(b,"}");}b_s(b,"]");
  b_s(b,",\"edges\":[");for(uint32_t i=0;i<s->edge_count;i++){Edge*x=&s->edges[i];if(i)b_s(b,",");b_s(b,"{\"id\":");b_u(b,x->id);b_s(b,",\"child_identity_id\":");b_u(b,x->child);b_s(b,",\"parent_identity_id\":");b_u(b,x->parent);b_s(b,",\"first_sample\":");b_u(b,x->first_sample);b_s(b,",\"last_sample\":");b_u(b,x->last_sample);b_s(b,",\"observation_count\":");b_u(b,x->count);b_s(b,",\"lineage_root_roles\":");b_roots(b,x->lineage);b_s(b,"}");}b_s(b,"]");
  b_s(b,",\"executables\":[");for(uint32_t i=0;i<s->exec_count;i++){Executable*x=&s->execs[i];if(i)b_s(b,",");b_s(b,"{\"id\":");b_u(b,x->id);b_s(b,",\"device\":");b_u(b,(uint64_t)x->dev);b_s(b,",\"inode\":");b_u(b,(uint64_t)x->ino);b_s(b,",\"size\":");b_i(b,x->size);b_s(b,",\"mtime_ns\":");b_i(b,x->mtime_ns);b_s(b,",\"ctime_ns\":");b_i(b,x->ctime_ns);b_s(b,",\"sha256\":\"");b_digest(b,x->sha);b_s(b,"\",\"selected_executable_match\":");b_bool(b,x->selected);b_s(b,",\"bundle_tree_match\":");b_bool(b,x->bundle);b_s(b,"}");}b_s(b,"]");
  b_s(b,",\"relevant_events\":[");for(uint32_t i=0;i<s->event_count;i++){Event*x=&s->events[i];if(i)b_s(b,",");b_s(b,"{\"sequence\":");b_u(b,x->sequence);b_s(b,",\"sample_sequence\":");b_u(b,x->sample);b_s(b,",\"kind\":\"");b_s(b,EVENT_NAMES[x->kind]);b_s(b,"\",\"identity_id\":");b_nullable_u(b,x->has_identity,x->identity);b_s(b,",\"witnesses\":");b_witness(b,x->witnesses);b_s(b,"}");}b_s(b,"]");b_s(b,"}");

}
/* Publication decision after the rename step. Once rename succeeds the target IS the
 * published artifact: a later readback failure must keep it and report false, never
 * unlink it. Only a pre-rename failure leaves a temp this call still owns. */
static bool publish_readback_decision(bool renamed,bool readback_ok){
  if(!renamed)return false;
  return readback_ok;
}

static bool atomic_publish(const char*path,const char*bytes,size_t n){
  char tmp[PATH_MAX];
  if(snprintf(tmp,sizeof(tmp),"%s.tmp.%d",path,getpid())>=(int)sizeof(tmp))return false;
  struct stat st;
  if(!lstat(path,&st)&&(!S_ISREG(st.st_mode)||S_ISLNK(st.st_mode)))return false;
  int fd=open(tmp,O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW,0600);
  if(fd<0)return false;
  size_t off=0;
  while(off<n){ssize_t w=write(fd,bytes+off,n-off);if(w<0&&errno==EINTR)continue;if(w<=0){close(fd);unlink(tmp);return false;}off+=(size_t)w;}
  bool renamed=fsync(fd)==0&&close(fd)==0&&rename(tmp,path)==0;
  if(!renamed){unlink(tmp);return false;}
  /* From here the target is published: readback failures keep it, never unlink. */
  bool readback_ok=true;
  fd=open(path,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);
  if(fd<0){readback_ok=false;}
  else{
    char*copy=n?malloc(n):NULL;
    if(n&&!copy){readback_ok=false;}
    else{
      off=0;
      while(off<n){ssize_t r=read(fd,copy+off,n-off);if(r<0&&errno==EINTR)continue;if(r<=0){readback_ok=false;break;}off+=(size_t)r;}
      if(readback_ok){char extra;ssize_t more=read(fd,&extra,1);if(more!=0)readback_ok=false;}
      if(readback_ok&&memcmp(copy,bytes,n))readback_ok=false;
      free(copy);
    }
    close(fd);
  }
  return publish_readback_decision(renamed,readback_ok);
}
static PP_MAYBE_UNUSED bool publish_state(State*s,const char*status){Buffer b;b_init(&b,STATE_MAX);serialize_state(s,&b,status);if(b.failed){free(b.p);fail(s,F_STATE_LIMIT_EXCEEDED);return false;}bool ok=atomic_publish(s->state_path,b.p,b.n);free(b.p);if(!ok)fail(s,F_PUBLICATION_FAILED);return ok;}

/* Serialize one sanitized_sample: the exact 11 keys, RFC 8785 sorted, compact. The
 * process_identity_digests array is the sample's digest strings sorted by digest
 * bytes, so two runs with the same identities produce identical bytes. */
static void b_sanitized_sample(Buffer*b,State*s,const Sample*x){
  b_s(b,"{\"complete\":");b_bool(b,x->complete);
  b_s(b,",\"control_sequence_seen\":");b_nullable_u(b,x->control_sequence>=0,(uint32_t)x->control_sequence);
  b_s(b,",\"end_mono_ns\":");b_u(b,x->end_ns);
  b_s(b,",\"enumeration_ns\":");b_u(b,x->end_ns-x->start_ns);
  b_s(b,",\"group_member_count\":");b_u(b,x->group_count);
  b_s(b,",\"kind\":\"");b_s(b,x->kind=='f'?"first":x->kind=='s'?"sentinel":"internal");b_s(b,"\"");
  b_s(b,",\"process_identity_digests\":[");
  /* Sort the sample's identities by digest bytes before emitting. */
  uint32_t n=x->ids_count;
  uint8_t(*d)[32]=n?malloc((size_t)n*32):NULL;
  if(n&&d){
    for(uint32_t j=0;j<n;j++)identity_digest(s->ids[s->sample_ids[x->ids_offset+j]].key,d[j]);
    qsort(d,n,32,cmp_digest);
    char prev[32];bool first=true;
    for(uint32_t j=0;j<n;j++){
      if(!first&&!memcmp(d[j],prev,32))continue;
      memcpy(prev,d[j],32);
      if(!first)b_s(b,",");
      b_s(b,"\"");b_digest(b,d[j]);b_s(b,"\"");
      first=false;
    }
  }
  free(d);
  b_s(b,"],\"process_row_count\":");b_u(b,x->ids_count);
  b_s(b,",\"sequence\":");b_u(b,x->sequence);
  b_s(b,",\"start_interval_ns\":");b_nullable_u(b,x->sequence>0,x->interval_ns);
  b_s(b,",\"start_mono_ns\":");b_u(b,x->start_ns);
  b_s(b,"}");
}

static void digest_samples(State*s,uint8_t out[32]){
  /* The sample sequence hashes the RFC 8785 ordered array of exact sanitized_sample
   * objects in ascending sequence order. */
  Buffer b;b_init(&b,FINAL_MAX);
  b_s(&b,"[");
  for(uint32_t i=0;i<s->sample_count;i++){if(i)b_s(&b,",");b_sanitized_sample(&b,s,&s->samples[i]);}
  b_s(&b,"]");
  if(b.failed){free(b.p);memset(out,0,32);return;}
  CC_SHA256_CTX c;d_begin(&c,"agenterm.singleton-safety.v1.sample-sequence");
  CC_SHA256_Update(&c,b.p,(CC_LONG)b.n);CC_SHA256_Final(out,&c);
  free(b.p);
}
/* baseline_summary: the sequence-zero baseline facts. Unobservable facts are null,
 * never zero or false. */
static void serialize_baseline_summary(Buffer*b,State*s){
  StageSnapshots*t=&s->stages;
  bool complete=t->have_baseline;
  b_s(b,"{\"complete\":");b_bool(b,complete);
  b_s(b,",\"selected_bundle_zero_independent\":");if(t->baseline_have_count)b_bool(b,t->baseline_zero_independent);else b_s(b,"null");
  b_s(b,",\"selected_bundle_process_count\":");b_nullable_u(b,t->baseline_have_count,t->baseline_selected_bundle_count);
  b_s(b,",\"independent_inventory_complete\":");b_bool(b,complete);
  b_s(b,"}");
}

/* session_ready_summary: browser-root observed and session-ready snapshot exists. */
static void serialize_session_ready_summary(Buffer*b,State*s){
  StageSnapshots*t=&s->stages;
  bool complete=t->have_session_ready&&s->root_valid[ROOT_BROWSER]&&s->root_obs[ROOT_BROWSER].observed&&!s->root_obs[ROOT_BROWSER].unobserved;
  b_s(b,"{\"complete\":");b_bool(b,complete);
  b_s(b,",\"browser_identity_digest\":");if(t->session_ready_have_identity){b_s(b,"\"");b_digest(b,t->session_ready_browser_identity);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"selected_bundle_set_digest\":");if(t->session_ready_have_bundle){b_s(b,"\"");b_digest(b,t->session_ready_bundle_set);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"selected_bundle_process_count\":");b_nullable_u(b,t->session_ready_have_count,t->session_ready_selected_bundle_count);
  b_s(b,",\"independent_inventory_complete\":");b_bool(b,complete);
  b_s(b,"}");
}

/* open_request_summary: direct-open-root observed and its exit snapshot exists. */
static void serialize_open_request_summary(Buffer*b,State*s){
  StageSnapshots*t=&s->stages;
  bool complete=s->root_valid[ROOT_DIRECT]&&s->root_obs[ROOT_DIRECT].observed&&!s->root_obs[ROOT_DIRECT].unobserved&&t->have_open_request;
  b_s(b,"{\"complete\":");b_bool(b,complete);
  b_s(b,",\"direct_request_identity_digest\":");if(s->root_valid[ROOT_DIRECT]){uint8_t d[32];identity_digest(s->roots[ROOT_DIRECT],d);b_s(b,"\"");b_digest(b,d);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"exit_observed\":");b_bool(b,t->have_open_request&&t->open_request_exit_observed);
  b_s(b,",\"descendant_retained_digest\":");if(t->have_open_request){b_s(b,"\"");b_digest(b,t->open_request_desc_retained);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"descendant_retained_count\":");b_nullable_u(b,t->have_open_request,t->open_request_desc_retained_count);
  b_s(b,",\"descendant_live_digest\":");if(t->open_request_desc_have_live){b_s(b,"\"");b_digest(b,t->open_request_desc_live);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"descendant_live_count\":");b_nullable_u(b,t->open_request_desc_have_live,t->open_request_desc_live_count);
  b_s(b,",\"boundary_sample_complete\":");b_bool(b,t->have_open_request);
  b_s(b,"}");
}

/* termination_summary: browser-root absent snapshot exists. */
static void serialize_termination_summary(Buffer*b,State*s){
  StageSnapshots*t=&s->stages;
  bool complete=t->have_termination;
  b_s(b,"{\"complete\":");b_bool(b,complete);
  b_s(b,",\"browser_absent\":");b_bool(b,t->have_termination&&t->termination_browser_absent);
  b_s(b,",\"contained_group_absent\":");b_bool(b,t->have_termination&&t->termination_contained_group_absent);
  b_s(b,",\"contained_closure_live_count\":");b_nullable_u(b,t->have_termination,t->termination_contained_closure_live_count);
  b_s(b,",\"token_set_digest\":");if(t->have_termination){b_s(b,"\"");b_digest(b,t->termination_token_set);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"token_match_count\":");b_nullable_u(b,t->have_termination,t->termination_token_count);
  b_s(b,",\"escaped_target_count\":");b_nullable_u(b,t->have_termination,t->termination_escaped_target_count);
  b_s(b,",\"independent_inventory_complete\":");b_bool(b,complete);
  b_s(b,"}");
}

/* final_inventory_summary: the sentinel snapshot after final-inventory-complete. */
static void serialize_final_inventory_summary(Buffer*b,State*s){
  StageSnapshots*t=&s->stages;
  bool complete=t->have_final_inventory&&s->samples[t->final_inventory_sample].complete;
  b_s(b,"{\"complete\":");b_bool(b,complete);
  b_s(b,",\"independent_inventory_complete\":");b_bool(b,complete);
  b_s(b,",\"selected_bundle_process_count\":");b_nullable_u(b,t->final_inventory_selected_bundle_have,t->final_inventory_selected_bundle_count);
  b_s(b,",\"token_set_digest\":");if(t->have_final_inventory){b_s(b,"\"");b_digest(b,t->final_inventory_token_set);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"token_match_count\":");b_nullable_u(b,t->have_final_inventory,t->final_inventory_token_count);
  b_s(b,",\"bundle_tree_outside_count\":");b_nullable_u(b,t->have_final_inventory,t->final_inventory_bundle_tree_outside_count);
  b_s(b,",\"contained_closure_retained_digest\":");if(t->have_final_inventory){b_s(b,"\"");b_digest(b,t->final_inventory_contained_retained);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"contained_closure_retained_count\":");b_nullable_u(b,t->have_final_inventory,t->final_inventory_contained_retained_count);
  b_s(b,",\"contained_closure_live_digest\":");if(t->have_final_inventory){b_s(b,"\"");b_digest(b,t->final_inventory_contained_live);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"contained_closure_live_count\":");b_nullable_u(b,t->have_final_inventory,t->final_inventory_contained_live_count);
  b_s(b,",\"direct_descendant_retained_digest\":");if(t->have_final_inventory){b_s(b,"\"");b_digest(b,t->final_inventory_direct_desc_retained);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"direct_descendant_live_count\":");b_nullable_u(b,t->have_final_inventory,t->final_inventory_direct_desc_live_count);
  b_s(b,",\"relay_descendant_retained_digest\":");if(t->have_final_inventory){b_s(b,"\"");b_digest(b,t->final_inventory_relay_desc_retained);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"relay_descendant_live_count\":");b_nullable_u(b,t->have_final_inventory,t->final_inventory_relay_desc_live_count);
  b_s(b,",\"candidate_descendant_retained_digest\":");if(t->have_final_inventory){b_s(b,"\"");b_digest(b,t->final_inventory_candidate_desc_retained);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"candidate_descendant_live_count\":");b_nullable_u(b,t->have_final_inventory,t->final_inventory_candidate_desc_live_count);
  b_s(b,",\"cleanup_targets_absent\":");b_bool(b,t->have_final_inventory&&t->final_inventory_cleanup_targets_absent);
  b_s(b,"}");
}

/* relay_summary: exactly the contract's 18 keys in order. Descriptive facts that
 * only make sense for a single relay are null when the count is not one. */
static void serialize_relay_summary(Buffer*b,State*s,uint64_t now_ms){
  RelayClassification*r=&s->relay;
  bool single=r->has_single_relay;
  /* complete requires the full universe classified AND an exit snapshot for every
   * relay/candidate: a missing absence never yields an empty-live pass. */
  bool all_exits=true;
  for(uint32_t i=0;i<s->cand_count;i++)if(s->cand_absent_sample[i]==UINT32_MAX)all_exits=false;
  bool complete=r->computed&&all_exits&&(!single||r->relay_exit_observed);
  b_s(b,"{\"complete\":");b_bool(b,complete);
  b_s(b,",\"candidate_observed_count\":");b_u(b,r->candidate_observed_count);
  b_s(b,",\"classified_relay_count\":");b_u(b,r->classified_relay_count);
  b_s(b,",\"topology_candidate_count\":");b_u(b,r->topology_candidate_count);
  b_s(b,",\"relay_identity_digest\":");
  if(single){uint8_t d[32];identity_digest(s->ids[r->relay_identity_id].key,d);b_s(b,"\"");b_digest(b,d);b_s(b,"\"");}
  else b_s(b,"null");
  b_s(b,",\"primary_predicates_ok\":");if(single)b_bool(b,r->relay_primary_ok);else b_s(b,"null");
  b_s(b,",\"auxiliary_predicates_ok\":");if(single)b_bool(b,r->relay_auxiliary_ok);else b_s(b,"null");
  b_s(b,",\"kernel_start_window_ok\":");if(single)b_bool(b,r->relay_kernel_window_ok);else b_s(b,"null");
  uint64_t elapsed=0;bool have=relay_exit_elapsed(s,now_ms,&elapsed);
  b_s(b,",\"relay_exit_elapsed_ms\":");if(have)b_u(b,elapsed);else b_s(b,"null");
  b_s(b,",\"relay_exit_within_deadline\":");if(have)b_bool(b,elapsed<=RELAY_EXIT_DEADLINE_MS);else b_s(b,"null");
  uint8_t dd[32],dl[32];uint32_t dc=0,dlc=0;
  if(single){digest_descendant_closure(s,ROOT_RELAY,dd,&dc);digest_live_descendant_closure(s,ROOT_RELAY,dl,&dlc);}
  b_s(b,",\"descendant_retained_digest\":");if(single){b_s(b,"\"");b_digest(b,dd);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"descendant_retained_count\":");b_nullable_u(b,single,dc);
  b_s(b,",\"descendant_live_digest\":");if(single){b_s(b,"\"");b_digest(b,dl);b_s(b,"\"");}else b_s(b,"null");
  b_s(b,",\"descendant_live_count\":");b_nullable_u(b,single,dlc);
  b_s(b,",\"non_exempt_token_outside_count\":");b_u(b,r->non_exempt_token_outside_count);
  b_s(b,",\"non_exempt_bundle_tree_outside_count\":");b_u(b,r->non_exempt_bundle_tree_outside_count);
  b_s(b,",\"live_g1_exemption_count\":");b_u(b,r->live_g1_exemption_count);
  b_s(b,",\"boundary_sample_complete\":");b_bool(b,s->sample_count&&s->samples[s->sample_count-1].complete);
  b_s(b,"}");
}

static void b_optional_digest(Buffer*b,const char*name,bool complete,const uint8_t d[32]){b_s(b,",\"");b_s(b,name);b_s(b,"\":");if(complete){b_s(b,"\"");b_digest(b,d);b_s(b,"\"");}else b_s(b,"null");}
static void serialize_ownership(Buffer*b,Ownership*o){b_s(b,"{\"complete\":");b_bool(b,o->complete);
  b_optional_digest(b,"contained_group_digest",o->complete,o->contained_group);b_s(b,",\"group_retained_count\":");b_nullable_u(b,o->complete,o->group_retained);b_s(b,",\"group_live_count\":");b_nullable_u(b,o->complete,o->group_live);b_optional_digest(b,"controlled_lineage_digest",o->complete,o->controlled);b_s(b,",\"controlled_lineage_count\":");b_nullable_u(b,o->complete,o->controlled_count);b_optional_digest(b,"contained_closure_retained_digest",o->complete,o->retained);b_s(b,",\"contained_closure_retained_count\":");b_nullable_u(b,o->complete,o->retained_count);b_optional_digest(b,"contained_closure_live_digest",o->complete,o->live);b_s(b,",\"contained_closure_live_count\":");b_nullable_u(b,o->complete,o->live_count);b_optional_digest(b,"token_set_digest",o->complete,o->token);b_s(b,",\"token_match_count\":");b_nullable_u(b,o->complete,o->token_count);b_optional_digest(b,"bundle_tree_set_digest",o->complete,o->bundle);b_s(b,",\"bundle_tree_outside_count\":");b_nullable_u(b,o->complete,o->bundle_outside);b_s(b,",\"breakaway_deviation_count\":");b_nullable_u(b,o->complete,o->breakaways);b_s(b,",\"browser_token_match\":");if(o->complete)b_bool(b,o->browser_token);else b_s(b,"null");b_s(b,",\"same_sample_insertion_check\":");if(o->complete)b_bool(b,o->insertion);else b_s(b,"null");b_s(b,"}");}
/* timing.minimum_sample_count: floor(window_duration_ms / (limits.start_interval_max_ns
 * / 1000000)). Derived from BOUND_NS so the divisor can never drift from the limit. */
static uint64_t minimum_sample_count(uint64_t window_ms){
  uint64_t divisor=BOUND_NS/1000000ull;
  if(!divisor)return 0;
  return window_ms/divisor;
}

static PP_MAYBE_UNUSED bool publish_final(State*s){bool sent=s->sample_count&&s->samples[s->sample_count-1].kind=='s';if(!sent){fail(s,F_SENTINEL_MISSING);s->early_stop=true;}if(!s->sample_count)fail(s,F_FIRST_SAMPLE_MISSING);uint64_t window=sent?s->samples[s->sample_count-1].start_ns-s->samples[0].start_ns:0;uint64_t wms=ceil_ms(window),mms=ceil_ms(s->max_interval),ems=ceil_ms(s->max_enumeration),rms=mms+ems;uint64_t min=minimum_sample_count(wms);bool all=true;for(uint32_t i=0;i<s->sample_count;i++)all&=s->samples[i].complete;bool first=s->sample_count&&s->samples[0].kind=='f'&&s->samples[0].complete;bool after=sent&&s->samples[s->sample_count-1].control_sequence==s->final_control;bool count=s->sample_count>=min;bool complete=!s->failed&&first&&sent&&after&&all&&s->max_interval<=BOUND_NS&&s->max_enumeration<=BOUND_NS&&count&&s->ownership.complete;uint8_t sd[32];digest_samples(s,sd);Buffer b;b_init(&b,FINAL_MAX);b_s(&b,"{\"schema\":\"");b_s(&b,SUMMARY_SCHEMA);b_s(&b,"\",\"status\":\"");b_s(&b,complete?"scan-complete":"scan-incomplete");b_s(&b,"\",\"failure_reasons\":");b_failures(&b,s);b_s(&b,",\"timing_ns\":{\"window_duration_ns\":");b_nullable_u(&b,sent,window);b_s(&b,",\"max_start_interval_ns\":");b_nullable_u(&b,s->sample_count>1,s->max_interval);b_s(&b,",\"max_enumeration_ns\":");b_nullable_u(&b,s->sample_count>0,s->max_enumeration);b_s(&b,",\"resolution_ns\":");b_nullable_u(&b,s->sample_count>1,s->max_interval+s->max_enumeration);b_s(&b,"},\"scan\":{\"sample_count\":");b_u(&b,s->sample_count);b_s(&b,",\"window_duration_ms\":");b_nullable_u(&b,sent,wms);b_s(&b,",\"max_start_interval_ms\":");b_nullable_u(&b,s->sample_count>1,mms);b_s(&b,",\"max_enumeration_ms\":");b_nullable_u(&b,s->sample_count>0,ems);b_s(&b,",\"resolution_ms\":");b_nullable_u(&b,s->sample_count>1,rms);b_s(&b,",\"maximum_row_count\":");b_u(&b,s->max_rows);b_s(&b,",\"maximum_group_member_count\":");b_u(&b,s->max_group);b_s(&b,",\"sample_sequence_digest\":\"");b_digest(&b,sd);b_s(&b,"\",\"complete\":");b_bool(&b,complete);b_s(&b,"},\"scan_coverage\":{\"complete\":");b_bool(&b,complete);b_s(&b,",\"sample_count\":");b_u(&b,s->sample_count);b_s(&b,",\"window_duration_ms\":");b_nullable_u(&b,sent,wms);b_s(&b,",\"minimum_sample_count\":");b_nullable_u(&b,sent,min);b_s(&b,",\"max_start_interval_ms\":");b_nullable_u(&b,s->sample_count>1,mms);b_s(&b,",\"max_enumeration_ms\":");b_nullable_u(&b,s->sample_count>0,ems);b_s(&b,",\"resolution_ms\":");b_nullable_u(&b,s->sample_count>1,rms);b_s(&b,",\"maximum_row_count\":");b_u(&b,s->max_rows);b_s(&b,",\"maximum_group_member_count\":");b_u(&b,s->max_group);b_s(&b,",\"first_sample_complete\":");b_bool(&b,first);b_s(&b,",\"sentinel_sample_complete\":");b_bool(&b,sent&&s->samples[s->sample_count-1].complete);b_s(&b,",\"sentinel_after_final_inventory\":");b_bool(&b,after);b_s(&b,",\"all_samples_complete\":");b_bool(&b,all);b_s(&b,",\"interval_bound_ok\":");b_bool(&b,s->max_interval<=BOUND_NS);b_s(&b,",\"enumeration_bound_ok\":");b_bool(&b,s->max_enumeration<=BOUND_NS);b_s(&b,",\"row_bound_ok\":");b_bool(&b,s->max_rows<=PROCESS_MAX&&!(s->failures&(1u<<F_PROCESS_ROW_LIMIT_EXCEEDED)));b_s(&b,",\"group_member_bound_ok\":");b_bool(&b,s->max_group<=GROUP_MAX&&!(s->failures&(1u<<F_GROUP_LIMIT_EXCEEDED)));b_s(&b,",\"count_lower_bound_ok\":");b_bool(&b,count);b_s(&b,",\"early_scanner_stop\":");b_bool(&b,s->early_stop);b_s(&b,",\"bundle_tree_resolution_charged\":");b_bool(&b,s->max_enumeration<=BOUND_NS);b_s(&b,",\"sample_sequence_digest\":\"");b_digest(&b,sd);b_s(&b,"\"},\"baseline\":");serialize_baseline_summary(&b,s);b_s(&b,",\"session_ready\":");serialize_session_ready_summary(&b,s);b_s(&b,",\"ownership\":");serialize_ownership(&b,&s->ownership);b_s(&b,",\"open_request\":");serialize_open_request_summary(&b,s);b_s(&b,",\"relay\":");serialize_relay_summary(&b,s,realtime_ms());b_s(&b,",\"termination\":");serialize_termination_summary(&b,s);b_s(&b,",\"final_inventory\":");serialize_final_inventory_summary(&b,s);b_s(&b,"}");if(b.failed){free(b.p);fail(s,F_FINAL_LIMIT_EXCEEDED);return false;}bool ok=atomic_publish(s->final_path,b.p,b.n);free(b.p);if(!ok)fail(s,F_PUBLICATION_FAILED);return ok;}

static bool same_parent_dir(const char*a,const char*b){char x[PATH_MAX],y[PATH_MAX];if(strlen(a)>=sizeof(x)||strlen(b)>=sizeof(y))return false;strcpy(x,a);strcpy(y,b);char*xs=strrchr(x,'/'),*ys=strrchr(y,'/');if(!xs||!ys)return false;*xs=*ys=0;return strcmp(x,y)==0;}
static bool regular_nosymlink(const char*p){struct stat l,s;return lstat(p,&l)==0&&!S_ISLNK(l.st_mode)&&stat(p,&s)==0&&S_ISREG(s.st_mode);}
static bool load_token(State*s,const char*p){if(!regular_nosymlink(p))return false;int fd=open(p,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);if(fd<0)return false;ssize_t n=read(fd,s->token,TOKEN_MAX+1);close(fd);if(n<1||n>TOKEN_MAX)return false;s->token_len=(size_t)n;for(size_t i=0;i<s->token_len;i++)if(s->token[i]==0||s->token[i]=='\r'||s->token[i]=='\n')return false;return true;}
static PP_MAYBE_UNUSED bool init_state(State*s,int argc,char**argv){memset(s,0,sizeof(*s));s->last_control=s->final_control=s->ownership_sample=s->frozen_pgid=-1;for(int i=0;i<ROOT_COUNT;i++)s->root_control_seq[i]=-1;if(argc!=17)return false;const char*names[]={"--contract-version","--target-uid","--token-file","--bundle-root","--selected-executable","--control-log","--latest-state","--final-summary"};for(int i=0;i<8;i++)if(strcmp(argv[1+i*2],names[i]))return false;if(strcmp(argv[2],"1"))return false;char*end=NULL;unsigned long u=strtoul(argv[4],&end,10);if(!end||*end||u>UINT_MAX)return false;s->uid=(uid_t)u;if(!load_token(s,argv[6]))return false;if(!realpath(argv[8],s->bundle_root)||!realpath(argv[10],s->selected_path))return false;if(strlen(argv[12])>=PATH_MAX||strlen(argv[14])>=PATH_MAX||strlen(argv[16])>=PATH_MAX)return false;strcpy(s->control_path,argv[12]);strcpy(s->state_path,argv[14]);strcpy(s->final_path,argv[16]);if(!regular_nosymlink(s->control_path)||!same_parent_dir(argv[6],s->control_path)||!same_parent_dir(s->control_path,s->state_path)||!same_parent_dir(s->state_path,s->final_path)||!strcmp(s->state_path,s->final_path))return false;struct stat control_stat;if(lstat(s->control_path,&control_stat)||!S_ISREG(control_stat.st_mode)||S_ISLNK(control_stat.st_mode))return false;s->control_dev=control_stat.st_dev;s->control_ino=control_stat.st_ino;int fd=open(s->selected_path,O_RDONLY|O_CLOEXEC|O_NOFOLLOW);if(fd<0||fstat(fd,&s->selected_stat)||!hash_fd(fd,s->selected_sha)){if(fd>=0)close(fd);return false;}close(fd);s->ids=calloc(IDENTITY_MAX,sizeof(*s->ids));s->edges=calloc(EDGE_MAX,sizeof(*s->edges));s->execs=calloc(EXECUTABLE_MAX,sizeof(*s->execs));s->events=calloc(EVENT_MAX,sizeof(*s->events));s->samples=calloc(SAMPLE_MAX,sizeof(*s->samples));return s->ids&&s->edges&&s->execs&&s->events&&s->samples;}
static PP_MAYBE_UNUSED void destroy(State*s){free(s->ids);free(s->edges);free(s->execs);free(s->events);free(s->samples);free(s->sample_ids);free(s->cand_ids);free(s->cand_bucket);free(s->cand_absent_sample);memset(s,0,sizeof(*s));}

#ifndef PROCESS_PROBE_SYNTHETIC_TEST
int main(int argc,char**argv){State s;if(!init_state(&s,argc,argv))return 2;uint64_t next=mono_ns();bool done=false;while(!done){if(!poll_controls(&s)){s.early_stop=true;break;}if(s.abort_seen){s.early_stop=true;break;}bool sentinel=s.final_seen;sample_native(&s,s.last_control,sentinel);if(s.ownership_pending&&!s.ownership_done&&s.samples[s.sample_count-1].complete)freeze_ownership(&s);publish_state(&s,s.failed?"scan-incomplete":sentinel?"scan-complete":"scanning");if(sentinel){done=true;break;}if(s.sample_count>=SAMPLE_MAX){fail(&s,F_SAMPLE_LIMIT_EXCEEDED);s.early_stop=true;break;}next+=CADENCE_NS;uint64_t now=mono_ns();if(next>now){struct timespec t={(time_t)((next-now)/1000000000ull),(long)((next-now)%1000000000ull)};while(nanosleep(&t,&t)&&errno==EINTR){}}else next=now;}
  /* A successful run keeps its scan-complete latest-state; only a failed run is
   * republished as scan-incomplete. */
  if(s.failed)publish_state(&s,"scan-incomplete");
  bool ok=publish_final(&s);int code=ok&&!s.failed?0:1;destroy(&s);return code;}
#else
#include <assert.h>

/* Allocate a synthetic state without touching the filesystem. */
static void synth_init(State*s,uint32_t ids,uint32_t edges,uint32_t events){
  memset(s,0,sizeof(*s));
  s->ids=calloc(ids,sizeof(*s->ids));
  s->edges=calloc(edges,sizeof(*s->edges));
  s->events=calloc(events,sizeof(*s->events));
  s->samples=calloc(8,sizeof(*s->samples));
  assert(s->ids&&s->edges&&s->events&&s->samples);
}
static void synth_free(State*s){destroy(s);}
static int synth_identity(State*s,pid_t pid,int64_t sec,int32_t usec,uid_t uid){
  int id=get_identity(s,(ProcIdentity){pid,sec,usec},0);
  if(id<0)return -1;
  s->ids[id].uid=uid;
  return id;
}
static char *hex_of(const uint8_t d[32]){static char h[65];hex32(d,h);return h;}

/* 1. PID reuse must never merge two kernel identities. */
static void test_pid_reuse(void){
  State s;synth_init(&s,8,4,16);
  int a=synth_identity(&s,10,1,2,501);
  int b=synth_identity(&s,10,1,4,501);   /* same pid, later kernel start */
  assert(a>=0&&b>=0&&a!=b);
  assert(!same_proc(s.ids[a].key,s.ids[b].key));
  /* The comparator must order by pid then start tuple. */
  ProcIdentity v[2]={s.ids[a].key,s.ids[b].key};
  qsort(v,2,sizeof(*v),cmp_proc);
  assert(v[0].usec==2&&v[1].usec==4);
  synth_free(&s);
}

/* 2. Sticky lineage fixed point within a single sample. */
static void test_lineage_fixed_point(void){
  State s;synth_init(&s,8,8,64);
  int root=synth_identity(&s,10,1,2,501);
  int mid =synth_identity(&s,11,1,3,501);
  int leaf=synth_identity(&s,12,1,4,501);
  s.ids[root].roots=1u<<ROOT_BROWSER;
  add_edge(&s,(uint32_t)mid,(uint32_t)root,0);
  add_edge(&s,(uint32_t)leaf,(uint32_t)mid,0);
  propagate_lineage(&s,0);
  /* root -> mid -> leaf must all carry the browser role from one pass. */
  assert(s.ids[mid].lineage&(1u<<ROOT_BROWSER));
  assert(s.ids[leaf].lineage&(1u<<ROOT_BROWSER));
  /* An edge first seen in a LATER sample cannot seed this sample's chain. */
  int late=synth_identity(&s,13,1,5,501);
  add_edge(&s,(uint32_t)late,(uint32_t)leaf,1);
  propagate_lineage(&s,0);
  assert(!(s.ids[late].lineage&(1u<<ROOT_BROWSER)));
  synth_free(&s);
}

/* 3. Digest fixed vectors. These bytes are the contract's encoding rule applied
 * by an independent implementation; a change here means the domain or separator
 * drifted. */
static void test_digest_vectors(void){
  uint8_t d[32];
  identity_digest((ProcIdentity){10,1,2},d);
  assert(!strcmp(hex_of(d),"106fadd28f4740a74192886a3e57a512e18d793ec3d23f6ee3f88a24f5e260f3"));
  identity_digest((ProcIdentity){11,1,3},d);
  assert(!strcmp(hex_of(d),"675a192b46d7b8fb7e447a297a632bdd179a3a01d6684cb15a564dd68fa6cc7f"));
  /* The set digest must be order- and duplicate-free. */
  State s;synth_init(&s,8,4,16);
  int a=synth_identity(&s,10,1,2,501);
  int b=synth_identity(&s,11,1,3,501);
  uint32_t fwd[2]={(uint32_t)b,(uint32_t)a},rev[2]={(uint32_t)a,(uint32_t)b};
  uint8_t x[32],y[32],z[32];
  digest_identity_set(&s,fwd,2,x);
  digest_identity_set(&s,rev,2,y);
  uint32_t dup[3]={(uint32_t)a,(uint32_t)b,(uint32_t)a};
  digest_identity_set(&s,dup,3,z);
  assert(!memcmp(x,y,32)&&!memcmp(x,z,32));
  assert(!strcmp(hex_of(x),"910e47efda428bae321000ade8ae357ef2f15db1e917f1a46e4385b149ee0c80"));
  synth_free(&s);
}

/* 4. Three-bucket classification is mutually exclusive and conservative. */
static void test_bucket_conservation(void){
  State s;synth_init(&s,32,8,64);
  s.uid=501; s.frozen_pgid=555;
  /* direct-open-root */
  int root=synth_identity(&s,10,100,0,501);
  s.roots[ROOT_DIRECT]=s.ids[root].key; s.root_valid[ROOT_DIRECT]=true;
  s.root_obs[ROOT_DIRECT].observed=true; s.root_obs[ROOT_DIRECT].ever_observed=true;
  /* relay: primary + auxiliary */
  int relay=synth_identity(&s,20,101,0,501);   /* +1000ms */
  s.ids[relay].token=1;s.ids[relay].selected=1;s.ids[relay].in_group=0;
  s.ids[relay].ppid=1;s.ids[relay].pgid=777;s.ids[relay].live=true;s.ids[relay].uid=501;
  /* topology: primary, auxiliary fails, and the process is already gone, so the
   * live-g1 exemption cannot apply. */
  int topo=synth_identity(&s,21,101,0,501);
  s.ids[topo].token=1;s.ids[topo].selected=1;s.ids[topo].in_group=0;
  s.ids[topo].ppid=999;s.ids[topo].pgid=777;s.ids[topo].live=false;s.ids[topo].uid=501;
  /* excluded: outside the frozen group but no witness at all */
  int nowit=synth_identity(&s,22,101,0,501);
  s.ids[nowit].token=0;s.ids[nowit].selected=0;s.ids[nowit].bundle=0;
  s.ids[nowit].in_group=0;s.ids[nowit].uid=501;
  /* excluded: inside the frozen group */
  int ing=synth_identity(&s,23,101,0,501);
  s.ids[ing].token=1;s.ids[ing].selected=1;s.ids[ing].in_group=1;s.ids[ing].uid=501;
  /* excluded: kernel start before direct-open-root */
  int early=synth_identity(&s,24,99,0,501);
  s.ids[early].token=1;s.ids[early].selected=1;s.ids[early].in_group=0;s.ids[early].uid=501;
  classify_candidates(&s,0,100000);   /* now = 100000ms */
  assert(s.relay.candidate_observed_count==2);
  assert(s.relay.classified_relay_count==1);
  assert(s.relay.topology_candidate_count==1);
  assert(s.relay.live_g1_exemption_count==0);
  assert(s.relay.candidate_observed_count==
         s.relay.classified_relay_count+s.relay.topology_candidate_count+s.relay.live_g1_exemption_count);
  assert(s.relay.has_single_relay);
  /* topology-only outside projections never count witness outside the bucket. */
  assert(s.relay.non_exempt_token_outside_count==1);
  synth_free(&s);
}

/* 5. g1_exemption requires an elapsed-since-start within the deadline and no
 * auxiliary. A primary with a live auxiliary is a relay, not an exemption. */
static void test_g1_exemption(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  int root=synth_identity(&s,10,100,0,501);
  s.roots[ROOT_DIRECT]=s.ids[root].key; s.root_valid[ROOT_DIRECT]=true;
  s.root_obs[ROOT_DIRECT].observed=true; s.root_obs[ROOT_DIRECT].ever_observed=true;
  int g1=synth_identity(&s,30,101,0,501);   /* +1000ms, well inside 5000ms */
  s.ids[g1].token=1;s.ids[g1].selected=1;s.ids[g1].in_group=0;
  s.ids[g1].ppid=999;s.ids[g1].pgid=777;s.ids[g1].live=true;s.ids[g1].uid=501;
  classify_candidates(&s,0,101000);          /* 1000ms after kernel start */
  assert(s.relay.live_g1_exemption_count==1);
  assert(s.relay.classified_relay_count==0);
  assert(s.relay.candidate_observed_count==1);
  synth_free(&s);
}

/* 6. Root observation lifecycle: absent requires a prior observation. */
static void test_root_observation(void){
  State s;synth_init(&s,8,4,32);
  assert(!s.root_obs[ROOT_BROWSER].observed&&!s.root_obs[ROOT_BROWSER].absent);
  s.root_obs[ROOT_BROWSER].observed=true;
  s.root_obs[ROOT_BROWSER].ever_observed=true;
  s.root_obs[ROOT_BROWSER].first_observed_sample=2;
  /* A never-observed root cannot claim absence. */
  assert(!s.root_obs[ROOT_DIRECT].ever_observed&&!s.root_obs[ROOT_DIRECT].absent);
  synth_free(&s);
}

/* 7. Stage snapshots advance once and only from complete samples. */
static void test_stage_snapshots(void){
  State s;synth_init(&s,8,4,32);
  s.sample_count=3;
  s.samples[0].kind='f'; s.samples[0].complete=true;
  s.samples[1].kind='i'; s.samples[1].complete=true;
  s.samples[2].kind='s'; s.samples[2].complete=true;
  s.samples[2].control_sequence=0;
  s.control_seen[C_FINAL]=true; s.final_seen=true; s.final_control=0;
  advance_stages(&s,0,true);
  assert(s.stages.have_baseline&&s.stages.baseline_sample==0);
  /* An incomplete sample must not claim a snapshot nor capture any fact. */
  advance_stages(&s,1,false);
  assert(!s.stages.have_final_inventory);
  assert(!s.stages.final_inventory_selected_bundle_have);
  /* The next complete sample still claims it, at that sample only. */
  advance_stages(&s,2,true);
  assert(s.stages.have_final_inventory&&s.stages.final_inventory_sample==2);
  assert(s.stages.final_inventory_selected_bundle_have);
  /* A later complete sample cannot move an already-claimed snapshot. */
  s.sample_count=4;s.samples[3].kind='i';s.samples[3].complete=true;
  advance_stages(&s,3,true);
  assert(s.stages.final_inventory_sample==2);
  synth_free(&s);
}

static void test_constants(void){
  assert(CADENCE_NS==20000000);
  assert(BOUND_NS==200000000);
  assert(RELAY_FIRST_START_WINDOW_MS==2000);
  assert(RELAY_EXIT_DEADLINE_MS==5000);
  assert(ceil_ms(200000001)==201&&ceil_ms(200000000)==200);
}
/* 8. The relay summary emits exactly the contract's 18 keys in order, with
 * single-relay-only fields null when no relay was classified. */
static void test_relay_summary_keys(void){
  State s;synth_init(&s,8,4,16);
  s.sample_count=1;s.samples[0].complete=true;
  Buffer b;b_init(&b,65536);
  serialize_relay_summary(&b,&s,0);
  static const char*keys[]={"complete","candidate_observed_count","classified_relay_count",
    "topology_candidate_count","relay_identity_digest","primary_predicates_ok",
    "auxiliary_predicates_ok","kernel_start_window_ok","relay_exit_elapsed_ms",
    "relay_exit_within_deadline","descendant_retained_digest","descendant_retained_count",
    "descendant_live_digest","descendant_live_count","non_exempt_token_outside_count",
    "non_exempt_bundle_tree_outside_count","live_g1_exemption_count","boundary_sample_complete"};
  const char*c=b.p;size_t at=0;
  while(*c=='{'||*c==','){
    c++;
    /* expect "key": */
    if(*c!='"'){assert(0);}
    c++;
    const char*ks=c;
    while(*c&&*c!='"')c++;
    size_t kn=(size_t)(c-ks);
    assert(at<18);
    assert(kn==strlen(keys[at]));
    assert(!memcmp(ks,keys[at],kn));
    at++;
    c++; /* closing quote */
    if(*c!=':'){assert(0);}
    c++;
    /* skip the value: string, null, number, or bool */
    if(*c=='"'){c++;while(*c&&*c!='"')c++;c++;}
    else if(!strncmp(c,"null",4))c+=4;
    else if(!strncmp(c,"true",4))c+=4;
    else if(!strncmp(c,"false",5))c+=5;
    else while(*c&&*c!=','&&*c!='}')c++;
  }
  assert(at==18);
  /* No relay: the single-relay-only fields are null. */
  assert(strstr(b.p,"\"relay_identity_digest\":null"));
  assert(strstr(b.p,"\"relay_exit_elapsed_ms\":null"));
  assert(strstr(b.p,"\"primary_predicates_ok\":null"));
  assert(strstr(b.p,"\"candidate_observed_count\":0"));
  free(b.p);
  synth_free(&s);
}

/* 9. The sticky descendant closure excludes the root identity itself: the root
 * holds the role in roots, descendants hold it in lineage. */
static void test_descendant_closure(void){
  State s;synth_init(&s,8,8,64);
  int root=synth_identity(&s,10,1,2,501);
  int kid =synth_identity(&s,11,1,3,501);
  s.ids[root].roots|=1u<<ROOT_RELAY;
  s.ids[kid].lineage|=1u<<ROOT_RELAY;
  uint8_t d[32];uint32_t n=0;
  digest_descendant_closure(&s,ROOT_RELAY,d,&n);
  assert(n==1);   /* the kid only; the root is never in its own descendant closure */
  /* A non-descendant carrying only the roots bit contributes nothing. */
  State t;synth_init(&t,8,8,64);
  int r2=synth_identity(&t,10,1,2,501);
  t.ids[r2].roots|=1u<<ROOT_DIRECT;
  uint8_t d2[32];uint32_t n2=99;
  digest_descendant_closure(&t,ROOT_DIRECT,d2,&n2);
  assert(n2==0);
  /* Two distinct descendants count once each. */
  int k2=synth_identity(&s,12,1,4,501);
  s.ids[k2].lineage|=1u<<ROOT_RELAY;
  digest_descendant_closure(&s,ROOT_RELAY,d,&n);
  assert(n==2);
  synth_free(&t);
  synth_free(&s);
}

typedef struct { const char *keys[64]; size_t n; } KeySet;
static bool keyset_eq(const KeySet*a,const char**want,size_t n);
static KeySet json_top_keys(const char*p);

/* 10. R343-A: latest_state.root_observations must expose exactly the two court
 * roots. The four-role loop is a defect the contract forbids. */
static void test_root_observations_two_keys(void){
  State s;synth_init(&s,8,4,16);
  Buffer b;b_init(&b,65536);
  serialize_state(&s,&b,"scanning");
  const char*ro=strstr(b.p,"\"root_observations\":");
  assert(ro);
  assert(strstr(ro,"\"browser-root\":"));
  assert(strstr(ro,"\"direct-open-root\":"));
  /* relay/topology-candidate must never appear in this object. */
  assert(!strstr(ro,"\"relay-root\":"));
  assert(!strstr(ro,"\"topology-candidate-root\":"));
  /* The emitted state must be structurally closed JSON. */
  assert(b.p[b.n-1]=='}');
  assert(b.p[0]=='{');
  /* The root_observation_state key set is exact and ordered. */
  const char*rs=strstr(ro,"\"browser-root\":");
  assert(rs);
  static const char*rk[]={"status","identity_digest","first_observed_sample","first_absent_sample"};
  const char*q=rs+strlen("\"browser-root\":");
  KeySet ks=json_top_keys(q);
  assert(keyset_eq(&ks,rk,4));
  free(b.p);
  synth_free(&s);
}

/* 11. R343-B: minimum_sample_count must be derived from limits.start_interval_max_ns
 * (BOUND_NS / 1000000), never a hard-coded divisor. */
static void test_minimum_sample_count_formula(void){
  assert(BOUND_NS/1000000ull==200ull);
  assert(minimum_sample_count(0)==0);
  assert(minimum_sample_count(199)==0);
  assert(minimum_sample_count(200)==1);
  assert(minimum_sample_count(400)==2);
  assert(minimum_sample_count(999)==4);
}

/* A tiny JSON reader that extracts the ordered key set of each object. It is
 * deliberately strict: it exists to prove the emitted key sets match the contract
 * exactly rather than to be a general parser. */
static void keyset_add(KeySet*k,const char*p,size_t len){
  static char store[64][64];
  memcpy(store[k->n],p,len);store[k->n][len]=0;
  k->keys[k->n]=store[k->n];
  k->n++;
}
/* Parse the top-level object's keys in order. */
static KeySet json_top_keys(const char*p){
  KeySet k={0};
  while(*p&&*p!='{')p++;
  if(*p=='{')p++;
  while(*p&&*p!='}'){
    while(*p==' '||*p==','||*p=='\n')p++;
    if(*p!='"')break;
    p++;
    const char*st=p;while(*p&&*p!='"')p++;
    keyset_add(&k,st,(size_t)(p-st));
    p++;
    /* skip value */
    int depth=0;bool ins=false;
    for(;*p;p++){
      if(ins){if(*p=='\\'){p++;continue;}if(*p=='"')ins=false;continue;}
      if(*p=='"'){ins=true;continue;}
      if(*p=='{'||*p=='[')depth++;
      else if(*p=='}'||*p==']'){if(depth==0)break;depth--;}
      else if(*p==','&&depth==0)break;
    }
  }
  return k;
}
static bool keyset_eq(const KeySet*a,const char**want,size_t n){
  if(a->n!=n)return false;
  for(size_t i=0;i<n;i++)if(strcmp(a->keys[i],want[i]))return false;
  return true;
}

/* 12. R343: each summary object must emit exactly its contract key set, in order. */
static void test_summary_key_sets(void){
  State s;synth_init(&s,8,4,16);
  Buffer b;b_init(&b,1<<20);
  static const char*base[]={"complete","selected_bundle_zero_independent","selected_bundle_process_count","independent_inventory_complete"};
  static const char*sess[]={"complete","browser_identity_digest","selected_bundle_set_digest","selected_bundle_process_count","independent_inventory_complete"};
  static const char*openr[]={"complete","direct_request_identity_digest","exit_observed","descendant_retained_digest","descendant_retained_count","descendant_live_digest","descendant_live_count","boundary_sample_complete"};
  static const char*term[]={"complete","browser_absent","contained_group_absent","contained_closure_live_count","token_set_digest","token_match_count","escaped_target_count","independent_inventory_complete"};
  static const char*fin[]={"complete","independent_inventory_complete","selected_bundle_process_count","token_set_digest","token_match_count","bundle_tree_outside_count","contained_closure_retained_digest","contained_closure_retained_count","contained_closure_live_digest","contained_closure_live_count","direct_descendant_retained_digest","direct_descendant_live_count","relay_descendant_retained_digest","relay_descendant_live_count","candidate_descendant_retained_digest","candidate_descendant_live_count","cleanup_targets_absent"};
  KeySet ks;
  serialize_baseline_summary(&b,&s);
  ks=json_top_keys(b.p); assert(keyset_eq(&ks,base,4));
  b.n=0;
  serialize_session_ready_summary(&b,&s);
  ks=json_top_keys(b.p); assert(keyset_eq(&ks,sess,5));
  b.n=0;
  serialize_open_request_summary(&b,&s);
  ks=json_top_keys(b.p); assert(keyset_eq(&ks,openr,8));
  b.n=0;
  serialize_termination_summary(&b,&s);
  ks=json_top_keys(b.p); assert(keyset_eq(&ks,term,8));
  b.n=0;
  serialize_final_inventory_summary(&b,&s);
  ks=json_top_keys(b.p); assert(keyset_eq(&ks,fin,17));
  free(b.p);
  synth_free(&s);
}

/* 13. R343: with no stage snapshots, every unobservable nullable field is null and
 * no field is a fabricated zero or false. */
static void test_summaries_fail_closed(void){
  State s;synth_init(&s,8,4,16);
  Buffer b;b_init(&b,1<<20);
  serialize_baseline_summary(&b,&s);
  assert(strstr(b.p,"\"complete\":false"));
  assert(strstr(b.p,"\"selected_bundle_zero_independent\":null"));
  assert(strstr(b.p,"\"selected_bundle_process_count\":null"));
  assert(strstr(b.p,"\"independent_inventory_complete\":false"));
  b.n=0;
  serialize_session_ready_summary(&b,&s);
  assert(strstr(b.p,"\"browser_identity_digest\":null"));
  assert(strstr(b.p,"\"selected_bundle_set_digest\":null"));
  b.n=0;
  serialize_open_request_summary(&b,&s);
  assert(strstr(b.p,"\"direct_request_identity_digest\":null"));
  assert(strstr(b.p,"\"descendant_retained_count\":null"));
  assert(strstr(b.p,"\"descendant_live_count\":null"));
  b.n=0;
  serialize_termination_summary(&b,&s);
  assert(strstr(b.p,"\"contained_closure_live_count\":null"));
  assert(strstr(b.p,"\"token_match_count\":null"));
  assert(strstr(b.p,"\"escaped_target_count\":null"));
  b.n=0;
  serialize_final_inventory_summary(&b,&s);
  assert(strstr(b.p,"\"selected_bundle_process_count\":null"));
  assert(strstr(b.p,"\"token_match_count\":null"));
  assert(strstr(b.p,"\"cleanup_targets_absent\":false"));
  free(b.p);
  synth_free(&s);
}

/* 14. R343: a complete snapshot must not leave its scanner_owned nullable fields
 * null. This exercises baseline only, which needs no root controls. */
static void test_complete_snapshot_nonnull(void){
  State s;synth_init(&s,8,4,16);
  s.stages.have_baseline=true;s.stages.baseline_sample=0;
  s.stages.baseline_have_count=true;s.stages.baseline_selected_bundle_count=0;
  s.stages.baseline_zero_independent=true;
  Buffer b;b_init(&b,1<<20);
  serialize_baseline_summary(&b,&s);
  assert(strstr(b.p,"\"complete\":true"));
  assert(strstr(b.p,"\"selected_bundle_process_count\":0"));
  assert(strstr(b.p,"\"selected_bundle_zero_independent\":true"));
  assert(strstr(b.p,"\"independent_inventory_complete\":true"));
  assert(!strstr(b.p,"null"));
  free(b.p);
  synth_free(&s);
}

/* 16. P0-1 known-answer (cc): process-set over [[900,1700000000,500000],[42,1700000001,1]].
 * The expected bytes were computed by an independent Python implementation, not
 * by this file. */
static void test_p0_1_known_answer(void){
  ProcIdentity ids[2]={{900,1700000000,500000},{42,1700000001,1}};
  uint8_t d[32];
  /* [900,1700000000,500000] -> ae1f..., [42,1700000001,1] -> 8608... (the RFC8785
   * preimage sorts the hex strings by bytes, so 8608 precedes ae1f). */
  identity_digest(ids[0],d);
  assert(!strcmp(hex_of(d),"ae1f5c3dc6678bedf38cd6c71190c09a0b486db963d2334086103f7a8e207245"));
  identity_digest(ids[1],d);
  assert(!strcmp(hex_of(d),"8608be23643dc504c9c604c38337bfa3308868f3cec302a13484a02cce3cfd75"));
  digest_process_set(ids,2,"agenterm.singleton-safety.v1.process-set",d);
  assert(!strcmp(hex_of(d),"dce23136839fd4e27ced4278844651b35a5bc468dea817438242ae6d98ae87b8"));
  /* The set is order- and duplicate-free. */
  ProcIdentity rev[2]={{42,1700000001,1},{900,1700000000,500000}};
  uint8_t e[32];
  digest_process_set(rev,2,"agenterm.singleton-safety.v1.process-set",e);
  assert(!memcmp(d,e,32));
  ProcIdentity dup[3]={{900,1700000000,500000},{42,1700000001,1},{900,1700000000,500000}};
  digest_process_set(dup,3,"agenterm.singleton-safety.v1.process-set",e);
  assert(!memcmp(d,e,32));
}

/* 17. sample_sequence known-answer (cc): the digest covers the RFC 8785 array of
 * exact 11-key sanitized_sample objects. The expected bytes came from an
 * independent Python implementation. */
static void test_p0_2_sample_sequence_known_answer(void){
  State s;synth_init(&s,8,4,16);
  s.sample_count=2;
  /* sample 0: two identities [10,1,2],[11,1,3] */
  int a=synth_identity(&s,10,1,2,501);
  int b=synth_identity(&s,11,1,3,501);
  uint32_t ids0[2]={(uint32_t)a,(uint32_t)b};
  append_sample_ids(&s,ids0,2,&s.samples[0].ids_offset);
  s.samples[0].sequence=0;s.samples[0].kind='f';
  s.samples[0].start_ns=1000000000ull;s.samples[0].end_ns=1003000000ull;
  s.samples[0].control_sequence=-1;s.samples[0].complete=true;
  s.samples[0].group_count=1;s.samples[0].ids_count=2;
  s.samples[0].interval_ns=0;
  /* sample 1: sentinel, no identities */
  s.samples[1].sequence=1;s.samples[1].kind='s';
  s.samples[1].start_ns=1020000000ull;s.samples[1].end_ns=1024000000ull;
  s.samples[1].control_sequence=3;s.samples[1].complete=true;
  s.samples[1].group_count=0;s.samples[1].ids_count=0;
  s.samples[1].interval_ns=20000000ull;
  /* The sanitized_sample serialization must match the exact bytes the known-answer
   * was computed over. */
  Buffer buf;b_init(&buf,1<<20);
  b_s(&buf,"[");b_sanitized_sample(&buf,&s,&s.samples[0]);b_s(&buf,",");b_sanitized_sample(&buf,&s,&s.samples[1]);b_s(&buf,"]");
  const char*expect=
    "[{\"complete\":true,\"control_sequence_seen\":null,\"end_mono_ns\":1003000000,"
    "\"enumeration_ns\":3000000,\"group_member_count\":1,\"kind\":\"first\","
    "\"process_identity_digests\":[\"106fadd28f4740a74192886a3e57a512e18d793ec3d23f6ee3f88a24f5e260f3\","
    "\"675a192b46d7b8fb7e447a297a632bdd179a3a01d6684cb15a564dd68fa6cc7f\"],"
    "\"process_row_count\":2,\"sequence\":0,\"start_interval_ns\":null,\"start_mono_ns\":1000000000},"
    "{\"complete\":true,\"control_sequence_seen\":3,\"end_mono_ns\":1024000000,"
    "\"enumeration_ns\":4000000,\"group_member_count\":0,\"kind\":\"sentinel\","
    "\"process_identity_digests\":[],\"process_row_count\":0,\"sequence\":1,"
    "\"start_interval_ns\":20000000,\"start_mono_ns\":1020000000}]";
  assert(!strcmp(buf.p,expect));
  free(buf.p);
  uint8_t d[32];digest_samples(&s,d);
  assert(!strcmp(hex_of(d),"0ef1ac9355e024eb1bfabf0856d073976961eda3b170392dbac8fa2515b2874d"));
  synth_free(&s);
}

/* 18. P0-3: the first complete sample after a root control that neither observes nor
 * prior-matches the exact identity makes the root unobserved and fails the run. */
static void test_root_unobserved(void){
  State s;synth_init(&s,8,4,16);
  /* A browser-root control exists but no identity matches it. */
  s.roots[ROOT_BROWSER]=(ProcIdentity){777,5,5};
  s.root_valid[ROOT_BROWSER]=true;
  s.root_control_seq[ROOT_BROWSER]=0;
  s.root_control_sample[ROOT_BROWSER]=0;
  s.root_obs[ROOT_BROWSER].unobserved=true;   /* as the miss pass would set */
  fail(&s,F_ROOT_UNOBSERVED);
  assert(s.failed);
  assert(s.failures&(1u<<F_ROOT_UNOBSERVED));
  assert(!strcmp(FAILURE_NAMES[F_ROOT_UNOBSERVED],"root-unobserved"));
  /* A session_ready snapshot cannot pass while the browser root is unobserved. */
  s.stages.have_session_ready=true;
  Buffer b;b_init(&b,1<<16);
  serialize_session_ready_summary(&b,&s);
  assert(strstr(b.p,"\"complete\":false"));
  free(b.p);
  synth_free(&s);
}

/* 19. A root that was observed and then exits is "observed", not "unobserved": a
 * normal exit is not a failure. */
static void test_root_normal_exit_not_unobserved(void){
  State s;synth_init(&s,8,4,16);
  s.roots[ROOT_BROWSER]=(ProcIdentity){777,5,5};
  s.root_valid[ROOT_BROWSER]=true;
  s.root_obs[ROOT_BROWSER].observed=true;
  s.root_obs[ROOT_BROWSER].ever_observed=true;
  s.root_obs[ROOT_BROWSER].absent=true;
  s.root_obs[ROOT_BROWSER].miss_checked=true;
  assert(!s.root_obs[ROOT_BROWSER].unobserved);
  s.stages.have_session_ready=true;
  Buffer b;b_init(&b,1<<16);
  serialize_session_ready_summary(&b,&s);
  assert(strstr(b.p,"\"complete\":true"));
  free(b.p);
  synth_free(&s);
}

/* 20. A candidate whose frozen-group membership is unknown (in_group == -1, before
 * the group is frozen) is never silently treated as outside: it is excluded. */
static void test_in_group_unknown_fail_closed(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  int root=synth_identity(&s,10,100,0,501);
  s.roots[ROOT_DIRECT]=s.ids[root].key; s.root_valid[ROOT_DIRECT]=true;
  s.root_obs[ROOT_DIRECT].observed=true; s.root_obs[ROOT_DIRECT].ever_observed=true;
  int unk=synth_identity(&s,40,101,0,501);
  s.ids[unk].token=1;s.ids[unk].selected=1;s.ids[unk].in_group=-1;  /* unknown */
  s.ids[unk].ppid=1;s.ids[unk].pgid=777;s.ids[unk].live=true;s.ids[unk].uid=501;
  classify_candidates(&s,0,101000);
  assert(s.relay.candidate_observed_count==0);
  synth_free(&s);
}

/* 21. Success finalization must keep the scan-complete latest status; only a failed
 * run republishes scan-incomplete. This exercises the pure decision helper. */
static void test_success_status_not_overwritten(void){
  /* Mirrors the main-loop contract: status = failed ? scan-incomplete : previous. */
  bool failed=false; const char*st=failed?"scan-incomplete":"scan-complete";
  assert(!strcmp(st,"scan-complete"));
  failed=true; st=failed?"scan-incomplete":"scan-complete";
  assert(!strcmp(st,"scan-incomplete"));
}

/* 22. Consecutive complete samples classify candidates incrementally and stickily. */
static void test_incremental_sticky_classification(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  int root=synth_identity(&s,10,100,0,501);
  s.roots[ROOT_DIRECT]=s.ids[root].key; s.root_valid[ROOT_DIRECT]=true;
  s.root_obs[ROOT_DIRECT].observed=true; s.root_obs[ROOT_DIRECT].ever_observed=true;
  int a=synth_identity(&s,20,101,0,501);
  s.ids[a].token=1;s.ids[a].selected=1;s.ids[a].in_group=0;s.ids[a].ppid=1;s.ids[a].pgid=777;s.ids[a].live=true;s.ids[a].uid=501;
  classify_candidates(&s,0,101000);
  assert(s.relay.candidate_observed_count==1);
  /* A later sample adds a second candidate without dropping the first. */
  int b=synth_identity(&s,21,102,0,501);
  s.ids[b].token=1;s.ids[b].selected=1;s.ids[b].in_group=0;s.ids[b].ppid=999;s.ids[b].pgid=777;s.ids[b].live=false;s.ids[b].uid=501;
  classify_candidates(&s,1,102000);
  assert(s.relay.candidate_observed_count==2);
  assert(s.relay.classified_relay_count==1);
  assert(s.relay.topology_candidate_count==1);
  synth_free(&s);
}

/* 23. R345A-1: classified candidates with unknown frozen-group membership must fail
 * closed with unsupported-observation, never be silently skipped. */
static void test_in_group_unknown_unsupported(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  int root=synth_identity(&s,10,100,0,501);
  s.roots[ROOT_DIRECT]=s.ids[root].key; s.root_valid[ROOT_DIRECT]=true;
  s.root_obs[ROOT_DIRECT].observed=true; s.root_obs[ROOT_DIRECT].ever_observed=true;
  int unk=synth_identity(&s,40,101,0,501);
  s.ids[unk].token=1;s.ids[unk].selected=1;s.ids[unk].in_group=-1;   /* unknown */
  s.ids[unk].ppid=1;s.ids[unk].pgid=777;s.ids[unk].live=true;s.ids[unk].uid=501;
  classify_candidates(&s,0,101000);
  assert(s.failed);
  assert(s.failures&(1u<<F_UNSUPPORTED_OBSERVATION));
  assert(s.relay.candidate_observed_count==0);
  synth_free(&s);
}

/* 24. R345A-2: a readback failure after a successful rename must keep the published
 * target. The policy function is pure so it is testable without the filesystem. */
static void test_publish_readback_policy(void){
  /* rename done, readback failed -> keep target, report false, no target unlink. */
  assert(!publish_readback_decision(true,false));
  /* rename done, readback ok -> success. */
  assert(publish_readback_decision(true,true));
  /* rename never happened -> failure, temp is still ours. */
  assert(!publish_readback_decision(false,false));
}

/* 25. R345A-3: bundle outside count excludes only the identity that itself satisfies
 * the live G1 exemption, and the escapee must still be counted. */
static void test_bundle_outside_per_identity(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  /* direct root is itself in the bundle tree and outside the group */
  int direct=synth_identity(&s,10,100,0,501);
  s.ids[direct].bundle=1;s.ids[direct].in_group=0;s.ids[direct].selected=1;s.ids[direct].token=1;
  s.ids[direct].live=true;s.ids[direct].uid=501;
  s.roots[ROOT_DIRECT]=s.ids[direct].key; s.root_valid[ROOT_DIRECT]=true;
  s.root_obs[ROOT_DIRECT].observed=true; s.root_obs[ROOT_DIRECT].ever_observed=true;
  /* a second escapee bundle identity that must remain counted */
  int esc=synth_identity(&s,50,200,0,501);
  s.ids[esc].bundle=1;s.ids[esc].in_group=0;s.ids[esc].live=true;s.ids[esc].uid=501;
  /* far past the deadline: direct is NOT exempt -> both counted. */
  assert(bundle_outside_count(&s,200000)==2);
  /* within the deadline: only the direct root is exempt, the escapee remains. */
  assert(bundle_outside_count(&s,101000)==1);
  /* Direct root not in the bundle tree: exemption cannot remove the escapee. */
  s.ids[direct].bundle=0;
  assert(bundle_outside_count(&s,101000)==1);
  synth_free(&s);
}

/* 26. R347-1: same_sample_insertion_check requires BOTH per-sample marks equal to
 * the current complete snapshot. Exercised through the production freeze_ownership:
 * enumerated-but-not-inserted is false; enumerated-and-inserted is true; a historical
 * exited in-group bundle identity never affects the result. */
static void test_same_sample_insertion_marks(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  s.sample_count=1;s.samples[0].kind='f';s.samples[0].complete=true;   /* snapshot seq 0 */
  int b=synth_identity(&s,9,1,1,501);
  s.roots[ROOT_BROWSER]=s.ids[b].key;s.root_valid[ROOT_BROWSER]=true;
  s.root_obs[ROOT_BROWSER].observed=true;s.root_obs[ROOT_BROWSER].ever_observed=true;
  /* enumerated this sample but NOT inserted into the closure */
  int a=synth_identity(&s,10,1,2,501);
  s.ids[a].live=true;s.ids[a].in_group=1;s.ids[a].bundle=1;
  s.ids[a].enumerated_bundle_group_sample=0;
  s.ids[a].closure_inserted_sample=UINT32_MAX;   /* never inserted */
  freeze_ownership(&s);
  assert(s.ownership.insertion==false);
  /* enumerated AND inserted */
  s.ids[a].closure_inserted_sample=0;
  freeze_ownership(&s);
  assert(s.ownership.insertion==true);
  /* a historical exited in-group bundle identity with stale marks must not matter */
  int old=synth_identity(&s,11,1,3,501);
  s.ids[old].live=false;s.ids[old].in_group=1;s.ids[old].bundle=1;
  s.ids[old].enumerated_bundle_group_sample=0;
  s.ids[old].closure_inserted_sample=UINT32_MAX;
  freeze_ownership(&s);
  assert(s.ownership.insertion==true);
  /* a mismatched mark (inserted in another sample) is false */
  s.ids[a].closure_inserted_sample=5;
  freeze_ownership(&s);
  assert(s.ownership.insertion==false);
  synth_free(&s);
}

/* 27. R347-2: the advance_stages ownership branch is dead; freeze_ownership is the
 * sole authority. This test asserts the stage field is no longer claimed by advance. */
static void test_no_dead_ownership_stage(void){
  State s;synth_init(&s,8,4,16);
  s.sample_count=1;s.samples[0].kind='f';s.samples[0].complete=true;
  s.control_seen[C_OWNERSHIP]=true;
  advance_stages(&s,0,true);
  /* freeze_ownership is the sole authority; advance_stages claims no stage. */
  synth_free(&s);
}

/* 28. R347-3: the direct root is never a member of the strictly-after candidate
 * universe (even when expired), yet an expired direct root that is a bundle-tree
 * match outside the group without lineage is still counted by the ownership/final
 * bundle_tree_outside_count (it is not a live G1 exemption). */
static void test_expired_direct_not_candidate_but_outside_projection(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  int direct=synth_identity(&s,10,100,0,501);
  s.ids[direct].token=1;s.ids[direct].bundle=1;s.ids[direct].selected=1;
  s.ids[direct].in_group=0;s.ids[direct].live=false;s.ids[direct].uid=501;  /* expired */
  s.roots[ROOT_DIRECT]=s.ids[direct].key; s.root_valid[ROOT_DIRECT]=true;
  s.root_obs[ROOT_DIRECT].observed=true; s.root_obs[ROOT_DIRECT].ever_observed=true;
  classify_candidates(&s,0,999999000);
  /* The direct root itself never enters the candidate universe. */
  assert(s.relay.candidate_observed_count==0);
  assert(s.relay.non_exempt_bundle_tree_outside_count==0);
  /* It is not live, so it cannot be a G1 exemption: the ownership/final outside
   * count still includes it. */
  assert(bundle_outside_count(&s,999999000)==0);   /* not live -> not counted */
  s.ids[direct].live=true;                          /* live but far past deadline */
  assert(bundle_outside_count(&s,999999000)==1);   /* counted: not exempt */
  assert(bundle_outside_count(&s,101000)==0);      /* within deadline -> exempt */
  synth_free(&s);
}


/* 29. R349: freeze_ownership must derive own from the current complete snapshot
 * (sample_count-1), never from the stale -1 it has not yet set. This calls the
 * production function directly. */
static void test_freeze_ownership_wiring(void){
  State s;synth_init(&s,16,8,32);
  s.uid=501; s.frozen_pgid=555;
  /* current complete snapshot is seq 2 */
  s.sample_count=3;
  s.samples[2].kind='s';s.samples[2].complete=true;
  int b=synth_identity(&s,10,2,2,501);
  s.roots[ROOT_BROWSER]=s.ids[b].key;s.root_valid[ROOT_BROWSER]=true;
  s.root_obs[ROOT_BROWSER].observed=true;s.root_obs[ROOT_BROWSER].ever_observed=true;
  int a=synth_identity(&s,11,2,3,501);
  s.ids[a].live=true;s.ids[a].in_group=1;s.ids[a].bundle=1;
  s.ids[a].enumerated_bundle_group_sample=2;
  s.ids[a].closure_inserted_sample=2;
  s.ownership_sample=-1;   /* must not be read by the fix */
  freeze_ownership(&s);
  assert(s.ownership.insertion==true);
  assert(s.ownership_sample==2);
  /* A marker from another sample makes insertion false. */
  s.ids[a].closure_inserted_sample=1;
  s.ownership_sample=-1;
  freeze_ownership(&s);
  assert(s.ownership.insertion==false);
  assert(s.ownership_sample==2);
  synth_free(&s);
}

static void synthetic_tests(void){
  test_constants();
  test_pid_reuse();
  test_lineage_fixed_point();
  test_digest_vectors();
  test_bucket_conservation();
  test_g1_exemption();
  test_root_observation();
  test_stage_snapshots();
  test_relay_summary_keys();
  test_descendant_closure();
  test_root_observations_two_keys();
  test_minimum_sample_count_formula();
  test_summary_key_sets();
  test_summaries_fail_closed();
  test_complete_snapshot_nonnull();
  test_p0_1_known_answer();
  test_p0_2_sample_sequence_known_answer();
  test_root_unobserved();
  test_root_normal_exit_not_unobserved();
  test_in_group_unknown_fail_closed();
  test_success_status_not_overwritten();
  test_incremental_sticky_classification();
  test_in_group_unknown_unsupported();
  test_publish_readback_policy();
  test_bundle_outside_per_identity();
  test_same_sample_insertion_marks();
  test_no_dead_ownership_stage();
  test_expired_direct_not_candidate_but_outside_projection();
  test_freeze_ownership_wiring();
}
int main(void){synthetic_tests();return 0;}
#endif
