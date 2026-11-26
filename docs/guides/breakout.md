# Breakout

Breakout opens several sessions as tiled Ghostty windows next to aoe, so you can watch and steer a handful at once. It needs macOS and [Ghostty](https://ghostty.org) on the machine whose screen you are looking at. aoe stays open.

## Use it

1. Mark sessions with `Space` (the keyboard help, `?`, lists the other mark keys).
2. Press `K` and choose **Break Out into Windows**.

Each window attaches to one session. Detaching (your tmux detach key) closes that window. Up to 9 sessions open at once; more than 6 asks first. **Show What Would Be Skipped** in the same menu lists the marked sessions that cannot open and why: structured-view sessions, sessions whose agent is not running, and sessions that are archived, in the trash, or being created or deleted.

While a session is broken out, its sidebar row shows a `⧉` marker and the preview header reads "In a Breakout window". On startup, local aoe rediscovers existing Ghostty Breakout windows and restores their markers without reopening or moving them. Discovery uses existing Automation permission and does not prompt for it. Host-tagged windows belong to aoe on another host and are not adopted locally. A window you close by hand keeps its marker until **Close Breakout Windows** or the next aoe restart.

## Closing

- Stopping, trashing or deleting a session from the sidebar closes its separate Breakout window. A `splits` window is not closed this way, and a stopped session keeps its marker there until **Close Breakout Windows**.
- **Close Breakout Windows** closes every Breakout window on this Mac, including ones aoe lost track of and ones opened for aoe on other hosts. It is in the `K` menu only while sessions are marked; with nothing marked, use "Close breakout windows" in the `Ctrl+K` palette.
- Before each new Breakout, windows whose session no longer exists are closed. Windows opened for another host are left alone, since this Mac cannot see that host's sessions. When aoe is started with `--profile`, this sweep is skipped, because it only loads that profile's sessions and could not tell another profile's live windows from orphans.
- aoe never launches Ghostty just to close windows.

Every Breakout window title starts with `aoe breakout · `. Separate windows end with ` · ` and the first 8 characters of the session id, windows for another host carry `@<host>` after the prefix, and a splits window is titled `aoe breakout · splits · <id8> · ...`. aoe finds its windows by these titles, so do not rename them.

## Layouts

```toml
[breakout]
layout = "windows"   # or "splits"
```

- `windows` (default): one window per session, tiled in a grid on the current screen.
- `splits`: one window with one pane per session, placed full-screen.

The setting is on the Interaction tab as **Breakout Layout**.

## Permissions

The first Breakout can raise two macOS consent prompts, both for the app that runs aoe (your terminal, or aoe itself for the Breakout agent):

- **Automation**: allows that app to control Ghostty. Grant it under System Settings > Privacy & Security > Automation. If it is not granted, Breakout fails within about 10 seconds with a message naming Automation.
- **Accessibility**: allows placing and raising windows. Grant it under System Settings > Privacy & Security > Accessibility. If placement fails, the windows still open untiled and the dialog shows the error.

## Window size and the aoe preview

A session's terminal follows whichever client used it most recently. While a Breakout window is attached, the aoe preview shows that window's size. If you also attach the same session inside aoe, the two trade size as you switch between them.

## aoe on another machine

Cross-host Breakout is new and best effort. When you run aoe over ssh on another host, a small agent on your Mac opens the windows.

On the Mac, allow the host in the global config (the agent ignores per-profile config):

```toml
# config.toml in the Mac's aoe app dir (~/.agent-of-empires/)
[breakout]
agent_hosts = ["bender"]
```

An entry must exactly match the remote machine's short hostname (`bender`, not `bender.local` or an ssh alias), because the Mac also uses it as the ssh destination. An empty list refuses every request. Then start the agent:

```sh
aoe breakout-agent
```

The agent prints the socket it listens on (`Breakout agent listening on ...`); that is the forward's local end. It sits in the Mac's aoe app dir, which is `~/.config/agent-of-empires` if that directory exists and `~/.agent-of-empires` otherwise.

The forward's remote end is the app dir of the remote aoe: `~/.config/agent-of-empires/breakout/agent.sock` on Linux (honoring `$XDG_CONFIG_HOME`), `~/.agent-of-empires/breakout/agent.sock` on a macOS remote (or the same path under `~/.config` if that app dir exists). sshd does not create the directory, so create it once on the remote host:

```sh
mkdir -p ~/.config/agent-of-empires/breakout && chmod 700 ~/.config/agent-of-empires/breakout
```

In `~/.ssh/config` on the Mac, forward the socket and share connections so each window does not pay a new handshake. `RemoteForward` does not expand `~`, so write both paths in full:

```
Host bender
    RemoteForward /home/YOU/.config/agent-of-empires/breakout/agent.sock /Users/YOU/.config/agent-of-empires/breakout/agent.sock
    ControlMaster auto
    ControlPath ~/.ssh/cm-%r@%h:%p
    ControlPersist 10m
```

On the remote host, let a new connection replace a stale forwarded socket (`/etc/ssh/sshd_config`):

```
StreamLocalBindUnlink yes
```

Then use Breakout in the remote aoe as usual. `aoe` must be on the remote host's PATH for non-interactive ssh. If the forward exists but the agent does not answer, the remote aoe says so and never opens windows on the remote host.

From the remote aoe, **Close Breakout Windows** and closing a window with its session do not reach the Mac. Close those windows on the Mac, or run **Close Breakout Windows** in an aoe running on the Mac.

### Security

The agent only opens windows that run `ssh -t <allowed host> aoe [--profile P] session attach <id>`, built on the Mac from validated values. No command or program from the request runs. But anything that can use the forwarded socket on the remote host can ask the Mac to open such windows: any process running as your user there, including agents inside aoe sessions, and root. Only list hosts you trust.
