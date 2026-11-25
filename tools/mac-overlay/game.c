/* A game's window on the engine's real window server (wsd), with no game: what tools/mac-overlay/check.py lays the overlay over. It is full screen as VRChat is, holds
 * the pointer (relative mouse, its cursor hidden) while it has the keyboard, and shows pictures at the size the server tells its clients, at 60 a second, which the server
 * stretches over the screen as it does a game's.
 *
 * Usage: game SOCKET FOLDER      FOLDER has wheel.bgra, qm.bgra, none.bgra of that size (the first line said is "size W H").
 * Commands on its standard input: show wheel|qm|none, stop (no more frames), go, window X Y W H (in pixels of the server), quit.
 * It says what it hears: focus 0|1, deactivated, key CODE DOWN, button N DOWN, resize W H, and every half second "shown N zero M" (frames macOS showed, and did not).
 * Built with the header of the engine's sources: cc -I <voile-lkd>/wsd. Nothing of it is in this repository.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
#include "wsd_protocol.h"

#define WIN 1u
#define BUFFERS 3
static int sock;
static uint32_t width, height;
static uint8_t *pictures[3], *pixels[BUFFERS];
static int busy[BUFFERS], now_shown = 2, going = 1;
static uint64_t shown, zero, serial;

static uint64_t now_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }
static void die(const char *why) { printf("failed %s (%d)\n", why, errno); exit(1); }

static void say(uint32_t type, const void *body, uint32_t n, int fd) {
  uint8_t bytes[sizeof(struct wsd_hdr) + WSD_MAX_PAYLOAD];
  struct wsd_hdr head = {type, n};
  memcpy(bytes, &head, sizeof head);
  if (n) memcpy(bytes + sizeof head, body, n);
  struct iovec part = {bytes, sizeof head + n};
  struct msghdr m = {.msg_iov = &part, .msg_iovlen = 1};
  char control[CMSG_SPACE(sizeof(int))];
  if (fd >= 0) {
    m.msg_control = control;
    m.msg_controllen = sizeof control;
    struct cmsghdr *c = CMSG_FIRSTHDR(&m);
    c->cmsg_level = SOL_SOCKET;
    c->cmsg_type = SCM_RIGHTS;
    c->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(c), &fd, sizeof fd);
  }
  if (sendmsg(sock, &m, 0) <= 0) die("sendmsg");
}
static void flag(uint32_t type, uint32_t value) { struct wsd_window_flag f = {WIN, value}; say(type, &f, sizeof f, -1); }

static uint8_t *read_picture(const char *folder, const char *name) {
  char path[1024];
  snprintf(path, sizeof path, "%s/%s.bgra", folder, name);
  size_t size = (size_t)width * height * 4;
  uint8_t *out = malloc(size);
  int fd = open(path, O_RDONLY);
  if (fd < 0 || !out || read(fd, out, size) != (ssize_t)size) die(path);
  close(fd);
  return out;
}

static void attach(uint32_t i) {
  size_t size = (size_t)width * height * 4;
  char name[40];
  snprintf(name, sizeof name, "/orbo-%d-%u", getpid(), i);
  int fd = shm_open(name, O_CREAT | O_EXCL | O_RDWR, 0600);
  if (fd < 0) die("shm_open");
  shm_unlink(name);
  if (ftruncate(fd, (off_t)size)) die("ftruncate");
  pixels[i] = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  if (pixels[i] == MAP_FAILED) die("mmap");
  struct wsd_buffer_attach a = {i + 1, width, height, width * 4, WSD_FMT_BGRA8, 0, 0, size};
  say(WSD_BUFFER_ATTACH, &a, sizeof a, fd);
}

static uint8_t in[1 << 16];
static size_t have;
/* Takes what the server said; returns 0 once it is gone. */
static int hear(void (*each)(uint32_t, const uint8_t *, uint32_t)) {
  struct pollfd p = {sock, POLLIN, 0};
  while (poll(&p, 1, 0) > 0) {
    ssize_t n = read(sock, in + have, sizeof in - have);
    if (n <= 0) return 0;
    have += (size_t)n;
    struct wsd_hdr h;
    while (have >= sizeof h && (memcpy(&h, in, sizeof h), have >= sizeof h + h.len)) {
      each(h.type, in + sizeof h, h.len);
      memmove(in, in + sizeof h + h.len, have - sizeof h - h.len);
      have -= sizeof h + h.len;
    }
  }
  return 1;
}
static void each(uint32_t type, const uint8_t *d, uint32_t n) {
  (void)n;
  switch (type) {
    case WSD_EV_HELLO: {
      const struct wsd_hello_reply *r = (const void *)d;
      if (!width) { width = r->display_width_px; height = r->display_height_px; printf("size %u %u\n", width, height); }
      break;
    }
    case WSD_EV_FOCUS: printf("focus %u\n", ((const struct wsd_window_flag *)d)->value); break;
    case WSD_EV_APP_DEACTIVATED: printf("deactivated\n"); break;
    case WSD_EV_KEY: { const struct wsd_ev_key *k = (const void *)d; printf("key %u %u\n", k->mac_keycode, k->flags & WSD_KEY_DOWN); break; }
    case WSD_EV_BUTTON: { const struct wsd_ev_button *b = (const void *)d; printf("button %u %u\n", b->button, b->down); break; }
    case WSD_EV_RELEASE: { uint32_t id = ((const struct wsd_ev_release *)d)->buffer_id; if (id && id <= BUFFERS) busy[id - 1] = 0; break; }
    case WSD_EV_PRESENTED: if (((const struct wsd_ev_presented *)d)->presented_ns) shown++; else zero++; break;
    case WSD_EV_ERROR: printf("error %d of request %u\n", ((const struct wsd_ev_error *)d)->code, ((const struct wsd_ev_error *)d)->request_type); break;
    default: break;
  }
}

static int command(char *line) {
  char name[16];
  int x, y, w, h;
  if (sscanf(line, "show %15s", name) == 1) now_shown = !strcmp(name, "wheel") ? 0 : !strcmp(name, "qm") ? 1 : 2;
  else if (!strncmp(line, "stop", 4)) going = 0;
  else if (!strncmp(line, "go", 2)) going = 1;
  else if (!strncmp(line, "hide", 4)) flag(WSD_WINDOW_MINIMIZE, 1);
  else if (!strncmp(line, "unhide", 6)) flag(WSD_WINDOW_MINIMIZE, 0);
  else if (sscanf(line, "window %d %d %d %d", &x, &y, &w, &h) == 4) {
    flag(WSD_WINDOW_FULLSCREEN, 0);
    struct wsd_window_frame f = {WIN, WSD_FRAME_MOVE | WSD_FRAME_SIZE, x, y, (uint32_t)w, (uint32_t)h};
    say(WSD_WINDOW_SET_FRAME, &f, sizeof f, -1);
  } else if (!strncmp(line, "full", 4)) flag(WSD_WINDOW_FULLSCREEN, 1);
  else if (!strncmp(line, "quit", 4)) return 0;
  printf("did %s", line);
  return 1;
}

int main(int argc, char **argv) {
  if (argc < 3) return 2;
  setvbuf(stdout, NULL, _IOLBF, 0);
  struct sockaddr_un a = {.sun_family = AF_UNIX};
  strncpy(a.sun_path, argv[1], sizeof a.sun_path - 1);
  sock = socket(AF_UNIX, SOCK_STREAM, 0);
  if (connect(sock, (struct sockaddr *)&a, sizeof a)) die("connect");
  struct wsd_hello hello = {WSD_PROTOCOL_VERSION, (uint32_t)getpid()};
  say(WSD_HELLO, &hello, sizeof hello, -1);
  for (int i = 0; i < 100 && !width; i++) { hear(each); usleep(20000); }
  if (!width) die("no hello");
  /* the pictures are made once the size is known */
  char line[128];
  if (!fgets(line, sizeof line, stdin)) return 1;
  const char *names[3] = {"wheel", "qm", "none"};
  for (int i = 0; i < 3; i++) pictures[i] = read_picture(argv[2], names[i]);
  for (uint32_t i = 0; i < BUFFERS; i++) attach(i);
  struct { struct wsd_window_create c; char title[16]; } made = {{WIN, width, height, WSD_STYLE_BORDERLESS}, "overlay check"};
  say(WSD_WINDOW_CREATE, &made, sizeof made.c + 13, -1);
  flag(WSD_WINDOW_SHOW, 1);
  flag(WSD_WINDOW_FULLSCREEN, 1);
  struct wsd_window w = {WIN};
  say(WSD_WINDOW_FOCUS, &w, sizeof w, -1);
  flag(WSD_CURSOR_VISIBLE, 0);
  flag(WSD_RELATIVE_MOUSE, 1);
  fcntl(0, F_SETFL, O_NONBLOCK);
  uint64_t due = now_ns(), said = due;
  size_t size = (size_t)width * height * 4, got = 0;
  for (;;) {
    if (!hear(each)) { printf("gone\n"); return 0; }
    ssize_t n = read(0, line + got, sizeof line - 1 - got);
    if (n == 0) break;
    if (n > 0) {
      got += (size_t)n;
      char *end;
      while ((end = memchr(line, '\n', got))) {
        char one[128];
        size_t k = (size_t)(end - line) + 1;
        memcpy(one, line, k);
        one[k] = 0;
        memmove(line, line + k, got - k);
        got -= k;
        if (!command(one)) goto out;
      }
    }
    uint64_t t = now_ns();
    if (t >= due) {
      due = (t - due > 50000000 ? t : due) + 16666667;
      for (int i = 0; going && i < BUFFERS; i++) {
        if (busy[i]) continue;
        memcpy(pixels[i], pictures[now_shown], size);
        ((uint32_t *)pixels[i])[0] = 0xff000000u | (uint32_t)(serial * 40503u & 0xffffff); /* never twice the same frame */
        busy[i] = 1;
        struct wsd_present p = {WIN, (uint32_t)i + 1, ++serial, now_ns()};
        say(WSD_PRESENT, &p, sizeof p, -1);
        break;
      }
    }
    if (t - said > 500000000) { said = t; printf("shown %llu zero %llu\n", (unsigned long long)shown, (unsigned long long)zero); }
    usleep(1000);
  }
out:
  flag(WSD_RELATIVE_MOUSE, 0);
  flag(WSD_CURSOR_VISIBLE, 1);
  flag(WSD_WINDOW_FULLSCREEN, 0);
  usleep(100000);
  say(WSD_QUIT, NULL, 0, -1);
  usleep(200000);
  return 0;
}
