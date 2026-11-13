# Password prompts

klemmbrett can ask for passwords on behalf of the usual desktop mechanisms,
so a minimal session needs no separate askpass, pinentry, or polkit agent:

| Agent | Answers | Set up with |
| --- | --- | --- |
| `secrets.SystemdAgent()` | `systemd-ask-password --user`: user services decrypting a key, `systemd-creds --user` | nothing else |
| `secrets.Askpass()` | ssh, sudo, and git asking through `klemmbrett-askpass` | `SSH_ASKPASS`, `SUDO_ASKPASS`, `GIT_ASKPASS` |
| `secrets.Pinentry()` | gpg-agent, through `klemmbrett-pinentry` | `pinentry-program` |
| `secrets.Polkit()` | `pkexec`, `systemctl`, `udisksctl`, and other actions needing authentication | nothing else; stop other polkit agents |

An agent goes where a widget's provider would. That widget asks the
agent's questions:

```python
import klemmbrett as kb
from klemmbrett import layouts, secrets

kb.widget("unlock", secrets.SystemdAgent())
kb.widget("ssh", secrets.Askpass())
kb.widget("gpg", secrets.Pinentry(), layout=layouts.secret)
kb.widget("admin", secrets.Polkit(), theme="nord")
```

Without a layout, an agent widget shows a centered card with the
question's title, message, and last error, then a masked input bar.
`layouts.secret` is the same card as a starting point for your own. Layout
text can use `{title}`, `{message}`, `{error}`, `{prompt}`, `{id}`, and
`{details.<name>}` (polkit's `{details.action_id}`, `{details.user}`, and
whatever details the action passes). `ui.Entry(mask="*")` changes the mask.
A layout that cannot ask the question, such as one without an input bar for
a password, is replaced by the built-in card.

## Customizing the card

Agent widgets take `theme` and `layout` like any widget (see
[theming](theming.md)). In a password card, a `Text` line whose template
comes out empty (no error yet, no title) is left out, and the `Prompt`
shows the asker's prompt (`Password:`) when it gives one, else its `text`.

A red frame for administrator prompts, so they stand out from the rest:

```python
from klemmbrett import secrets, themes, ui
from klemmbrett.style import Style, Theme

alarm = themes.nord | Theme(
    window=Style(border=2, border_color="#bf616a", border_radius=8, padding=8),
    prompt=Style(text_color="#bf616a"),
)
kb.widget("polkit", secrets.Polkit(), theme=alarm)
```

A polkit card that says which action and user it is about:

```python
def admin_card(ctx):
    line = Style(padding=(2, 10))
    return ui.Window(
        ui.Text("{title}", style=line, text_color="#ebcb8b"),
        ui.Text("{message}", style=line),
        ui.Text("{details.action_id} as {details.user}", style=line, text_color="#888"),
        ui.Text("{error}", style=line, text_color="#bf616a"),
        ui.InputBar(ui.Prompt(), ui.Entry(mask="✱")),
        location="center",
    )

kb.widget("polkit", secrets.Polkit(), layout=admin_card, theme=alarm)
```

Templates naming details a question does not have are kept as written,
so keep `{details.…}` to widgets whose agent sets them. When polkit lets
several users authenticate, it asks which one first; a layout without a
list, like this one, falls back to the built-in card for that step.

A quiet card at the top of the screen for systemd services, and a wider
mask for ssh:

```python
quiet = Theme(window=Style(width=420, location="north"), entry=Style(text_color="#a3be8c"))

kb.widget("unlock", secrets.SystemdAgent(), theme=themes.mbra | quiet)
kb.widget("askpass", secrets.Askpass(), layout=lambda ctx: layouts.secret(ctx, mask="● "))
```

## What stays private

Answers go back to the asker only. They never reach Python, `klemmbrett
watch` and other subscribers, handlers, history, usage, or the journal, and
the popup wipes what was typed once it is sent. That is why agent
widgets take no `on_select`, `on_highlight`, `multi`, `search`, `match`, or
`rank`.

While a password prompt is open, `Show` and `Prompt` are turned away
(`Busy`), so nothing can replace it. Questions arriving meanwhile wait their
turn. A question disappears when its asker gives up: when systemd's
deadline passes or the asking process exits, polkit cancels, or a
`timeout` runs out.

## Routing

`route` picks the widget per question, from what is known about it. It
returns a widget name, or `None` for the agent's own widget:

```python
def admin(question: secrets.Question) -> str | None:
    if question.action_id.startswith("org.freedesktop.udisks2."):
        return "mount"
    return None

kb.widget("admin", secrets.Polkit(route=admin))
kb.widget("mount", layout=layouts.secret, location="pointer")
```

A `Question` has `kind`, `id`, `title`, `message`, `details`, `pid`, and
the shortcuts `action_id` and `program`. A route that fails, or names an
unknown widget, is logged and the agent's widget asks.

At most one widget holds a `SystemdAgent` or a `Polkit`. Several may hold
`Askpass` or `Pinentry`: the first configured answers, unless the asking
program's environment sets `KLEMMBRETT_WIDGET`:

```python
kb.widget("ssh", secrets.Askpass())
kb.widget("git", secrets.Askpass(), layout=git_card)
```

```sh
git config --global core.askPass 'env KLEMMBRETT_WIDGET=git klemmbrett-askpass'
```

## Recipes

### Askpass for ssh, sudo, and git

`~/.config/environment.d/askpass.conf`:

```ini
SSH_ASKPASS=klemmbrett-askpass
SSH_ASKPASS_REQUIRE=prefer
SUDO_ASKPASS=klemmbrett-askpass
GIT_ASKPASS=klemmbrett-askpass
```

Then `sudo -A`, or `alias sudo='sudo -A'`. ssh's host key question and
git's user name question are shown as you type them; passwords are masked.
`SSH_ASKPASS_PROMPT=confirm` (as `ssh-add -c` sets) asks for Enter or
Escape. `kb askpass PROMPT` is the same program.

### gpg-agent

`~/.gnupg/gpg-agent.conf`:

```text
pinentry-program /home/me/.local/bin/klemmbrett-pinentry
```

Use the full path from `command -v klemmbrett-pinentry`, then
`gpgconf --kill gpg-agent`. Without a running daemon, or without a
`Pinentry` widget, every request is cancelled.

### A user service that needs a key

```ini
# ~/.config/systemd/user/backup.service
[Unit]
Description=Nightly backup

[Service]
Type=oneshot
Environment=RESTIC_PASSWORD_COMMAND="systemd-ask-password --user --id=backup:repo 'Backup passphrase:'"
ExecStart=restic backup %h/Documents
```

The `SystemdAgent` widget asks when the service starts. Answering also
works for `systemd-creds --user decrypt` and anything else using
`$XDG_RUNTIME_DIR/systemd/ask-password`.

### polkit

`secrets.Polkit()` registers klemmbrett as the session's authentication
agent, so polkit-gnome, lxpolkit, and the like must not run. The agent
needs a login session (`XDG_SESSION_ID`, or logind's display session for
your user). When an action lists several users who may authenticate (the
`wheel` group, say), klemmbrett asks which first, with you at the top.
After three failed passwords the action is denied.

## Over Varlink

`klemmbrett-askpass` and `klemmbrett-pinentry` never read the config; they
call `AskSecret` on the daemon's socket, which only its user can open. Other
programs can do the same; see [`rpc.md`](rpc.md).
