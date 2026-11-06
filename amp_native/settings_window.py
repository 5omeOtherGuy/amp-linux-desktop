"""App Settings: a libadwaita preferences window with the Mac app's six sections."""

import subprocess
from pathlib import Path

from gi.repository import Adw, Gdk, GLib, Gtk

from . import config
from .runner import find_cli

TABS = [
    ("general", "General", "emblem-system-symbolic"),
    ("account", "Account", "avatar-default-symbolic"),
    ("runner", "Runner", "computer-symbolic"),
    ("notifications", "Notifications", "preferences-system-notifications-symbolic"),
    ("updates", "Updates", "software-update-available-symbolic"),
    ("about", "About", "help-about-symbolic"),
]
ICON_CHOICES = [("dark", "Dark"), ("oled", "OLED"), ("light", "Light"), ("puck", "Puck")]


def switch_row(title, subtitle, active, on_toggle):
    row = Adw.ActionRow(title=title, subtitle=subtitle or "")
    switch = Gtk.Switch(active=active, valign=Gtk.Align.CENTER)
    switch.connect("notify::active", lambda s, _p: on_toggle(s.get_active()))
    row.add_suffix(switch)
    row.set_activatable_widget(switch)
    return row, switch


def combo_row(title, options, selected, on_select):
    row = Adw.ComboRow(title=title, model=Gtk.StringList.new([label for _, label in options]))
    values = [value for value, _ in options]
    row.set_selected(values.index(selected) if selected in values else 0)
    row.connect("notify::selected", lambda r, _p: on_select(values[r.get_selected()]))
    return row


class _Page:
    """Collects groups and plain widgets, then becomes an Adw.PreferencesPage."""

    def __init__(self):
        self.groups = []

    def append(self, widget):
        if not isinstance(widget, Adw.PreferencesGroup):
            wrapper = Adw.PreferencesGroup()
            wrapper.add(widget)
            widget = wrapper
        self.groups.append(widget)


def new_page():
    return _Page()


def group(title=None, description=None):
    g = Adw.PreferencesGroup()
    if title:
        g.set_title(title)
    if description:
        g.set_description(description)
    return g


class SettingsWindow(Adw.PreferencesWindow):
    def __init__(self, app):
        super().__init__(application=app, title="Settings", search_enabled=False)
        self.app = app
        self.settings = app.settings
        self.set_default_size(860, 680)
        self.pages = {}
        for key, label, icon in TABS:
            built = getattr(self, f"_build_{key}")()
            page = Adw.PreferencesPage(title=label, icon_name=icon, name=key)
            for g in built.groups:
                page.add(g)
            self.pages[key] = page
            self.add(page)
        self.connect("notify::visible-page", lambda *_: self._page_changed())

    def show_tab(self, key):
        if key in self.pages:
            self.set_visible_page(self.pages[key])
        self._page_changed()
        self.present()

    def _page_changed(self):
        page = self.get_visible_page()
        key = page.get_name() if page else None
        if key == "account":
            self.refresh_account()
        elif key == "runner":
            self.refresh_runner()

    # ---- General -----------------------------------------------------------------------------

    def _build_general(self):
        page = new_page()
        g = group()
        s = self.settings
        g.add(switch_row("Show Unread Count in Launcher", "The number of unread threads on Amp’s launcher icon.",
                         s["show_unread_count"], lambda v: (s.set("show_unread_count", v), self.app.refresh_badge()))[0])
        g.add(switch_row("Warn Before Quitting", "Hold Ctrl+Q to quit, so a stray press keeps Amp open.",
                         s["warn_before_quitting"], lambda v: s.set("warn_before_quitting", v))[0])
        g.add(switch_row("Open at Login", None, s["open_at_login"], self.app.set_open_at_login)[0])
        g.add(combo_row("Appearance", [("system", "System"), ("light", "Light"), ("dark", "Dark")],
                        s["appearance"], self.app.set_appearance))
        zooms = [(f, f"{round(f * 100)}%") for f in config.ZOOM_FACTORS]
        self.zoom_row = combo_row("Zoom", zooms, config.nearest_zoom(s["zoom"]), self.app.set_zoom)
        g.add(self.zoom_row)
        page.append(g)

        icons = [(key, label, config.icon_file(key)) for key, label in ICON_CHOICES]
        icons = [icon for icon in icons if icon[2]]
        if icons:
            g2 = group("App Icon", "Shows in the launcher and app switcher while Amp is open.")
            flow = Gtk.FlowBox(selection_mode=Gtk.SelectionMode.NONE, max_children_per_line=4,
                               min_children_per_line=4, homogeneous=True, halign=Gtk.Align.CENTER)
            flow.add_css_class("amp-icon-picker")
            self.icon_buttons = {}
            first = None
            for key, label, path in icons:
                button = Gtk.ToggleButton()
                button.add_css_class("amp-icon-choice")
                box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
                picture = Gtk.Picture.new_for_filename(str(path))
                picture.set_size_request(88, 88)
                picture.set_can_shrink(True)
                box.append(picture)
                box.append(Gtk.Label(label=label))
                button.set_child(box)
                if first:
                    button.set_group(first)
                else:
                    first = button
                button.set_active(s["app_icon"] == key)
                button.connect("toggled", lambda b, k: b.get_active() and self.app.set_app_icon(k), key)
                flow.append(button)
            card = Gtk.Box(css_classes=["card"])
            card.append(flow)
            flow.set_hexpand(True)
            flow.set_margin_top(16)
            flow.set_margin_bottom(16)
            g2.add(card)
            page.append(g2)
        return page

    # ---- Account -----------------------------------------------------------------------------

    def _build_account(self):
        page = new_page()
        self.account_group = group()
        page.append(self.account_group)
        self.account_rows = []
        return page

    def refresh_account(self):
        for row in self.account_rows:
            self.account_group.remove(row)
        self.account_rows = []
        state = self.app.main_window().app_state if self.app.main_window() else {}
        account = state.get("signedInAccount")
        if not account:
            card = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, css_classes=["card"],
                           margin_top=0)
            icon = Gtk.Image.new_from_icon_name("avatar-default-symbolic")
            icon.set_pixel_size(52)
            icon.set_margin_top(30)
            icon.add_css_class("dim-label")
            card.append(icon)
            title = Gtk.Label(label="Not Signed In", css_classes=["title-3", "dim-label"], margin_top=10)
            card.append(title)
            card.append(Gtk.Label(label="Sign in to Amp to see your account here.", css_classes=["dim-label"]))
            button = Gtk.Button(label="Open Amp to Sign In", halign=Gtk.Align.CENTER,
                                css_classes=["suggested-action"], margin_top=14, margin_bottom=30)
            button.connect("clicked", lambda *_: self.app.present_main())
            card.append(button)
            self.account_group.add(card)
            self.account_rows.append(card)
            return
        accounts = state.get("deviceAccounts") or [account]
        for entry in accounts:
            current = entry.get("userID") == account.get("userID")
            row = Adw.ActionRow(title=entry.get("username") or entry.get("email") or "",
                                subtitle=entry.get("email") or "")
            avatar = Adw.Avatar(size=32, text=entry.get("username") or entry.get("email") or "?", show_initials=True)
            row.add_prefix(avatar)
            if current:
                check = Gtk.Image.new_from_icon_name("object-select-symbolic")
                check.add_css_class("accent")
                row.add_suffix(check)
            else:
                button = Gtk.Button(label="Switch", valign=Gtk.Align.CENTER)
                button.connect("clicked", lambda _b, uid: self.app.main_window().app_command("switchAccount", userID=uid),
                               entry.get("userID"))
                row.add_suffix(button)
            self.account_group.add(row)
            self.account_rows.append(row)
        add = Adw.ActionRow(title="Add Account…", activatable=True)
        add.add_prefix(Gtk.Image.new_from_icon_name("list-add-symbolic"))
        add.connect("activated", lambda *_: (self.app.present_main(), self.app.main_window().app_command("addAccount")))
        self.account_group.add(add)
        self.account_rows.append(add)

    # ---- Runner ------------------------------------------------------------------------------

    def _build_runner(self):
        page = new_page()
        s = self.settings
        g = group()
        row = Adw.ActionRow(title="Use This Computer as a Runner")
        icon = Gtk.Image.new_from_icon_name("computer-symbolic")
        icon.set_pixel_size(32)
        row.add_prefix(icon)
        self.runner_switch = Gtk.Switch(active=s["runner_enabled"], valign=Gtk.Align.CENTER)
        self.runner_switch.connect("notify::active", lambda sw, _p: self._runner_setting("runner_enabled", sw.get_active()))
        row.add_suffix(self.runner_switch)
        row.set_activatable_widget(self.runner_switch)
        g.add(row)
        self.runner_status = row

        name = Adw.EntryRow(title="Runner Name", text=s["runner_name"], show_apply_button=True)
        name.connect("apply", lambda r: self._runner_setting("runner_name", r.get_text().strip() or s["runner_name"]))
        g.add(name)
        g.add(switch_row("Share This Computer’s Display",
                         "Threads running here can show and control this computer’s screen in their Desktop tab.",
                         s["runner_desktop"], lambda v: self._runner_setting("runner_desktop", v))[0])
        g.add(switch_row("Allow Terminal Access",
                         "Threads running here can open a shell on this computer in their Terminal tab.",
                         s["runner_terminal"], lambda v: self._runner_setting("runner_terminal", v))[0])
        g.add(switch_row("Keep This Computer Awake", "Prevent sleep while the runner is on.",
                         s["runner_keep_awake"], lambda v: self._runner_setting("runner_keep_awake", v))[0])
        page.append(g)

        where = group("Where Threads Can Run")
        home = Adw.ActionRow(title="Home Directory", subtitle="~")
        home.add_prefix(Gtk.Image.new_from_icon_name("user-home-symbolic"))
        home_switch = Gtk.Switch(active=s["runner_home"], valign=Gtk.Align.CENTER)
        home_switch.connect("notify::active", lambda sw, _p: self._runner_setting("runner_home", sw.get_active()))
        home.add_suffix(home_switch)
        home.set_activatable_widget(home_switch)
        where.add(home)
        self.discover_row = Adw.ActionRow(title="All Projects Under Directories")
        self.discover_row.add_prefix(Gtk.Image.new_from_icon_name("folder-saved-search-symbolic"))
        choose = Gtk.Button(label="Choose…", valign=Gtk.Align.CENTER)
        choose.connect("clicked", self._choose_folder, "runner_discover_dirs")
        clear = Gtk.Button(icon_name="edit-clear-symbolic", valign=Gtk.Align.CENTER, css_classes=["flat"])
        clear.set_tooltip_text("Stop Looking")
        clear.connect("clicked", lambda *_: self._runner_setting("runner_discover_dirs", []) or self._refresh_runner_paths())
        self.discover_row.add_suffix(clear)
        self.discover_row.add_suffix(choose)
        where.add(self.discover_row)
        self.new_dirs_row = Adw.ActionRow(title="New Projects Folder")
        self.new_dirs_row.add_prefix(Gtk.Image.new_from_icon_name("folder-new-symbolic"))
        new_choose = Gtk.Button(label="Choose…", valign=Gtk.Align.CENTER)
        new_choose.connect("clicked", self._choose_folder, "runner_new_dirs")
        self.new_dirs_row.add_suffix(new_choose)
        where.add(self.new_dirs_row)
        page.append(where)

        self.network_group = group()
        self.network_group.add(switch_row(
            "Share This Computer’s Network", "Let your orbs browse the web through this computer’s connection.",
            bool(self.app.runner.get("runner_share_network")),
            lambda v: self._runner_setting("runner_share_network", v))[0])
        page.append(self.network_group)
        self._refresh_runner_paths()

        self.dirs_group = group("Directories", "Add a project’s Git checkout. Threads start in the directory you add.")
        add = Gtk.Button(icon_name="list-add-symbolic", valign=Gtk.Align.CENTER, css_classes=["flat"])
        add.set_tooltip_text("Add Directory…")
        add.connect("clicked", self._add_directory)
        self.dirs_group.set_header_suffix(add)
        self.dir_rows = []
        page.append(self.dirs_group)
        self._refresh_dirs()
        return page

    def _runner_setting(self, key, value):
        if self.app.runner.get(key) == value:
            return
        self.settings.set(key, value)
        if key == "runner_enabled" or self.settings["runner_enabled"]:
            self.app.runner.apply()

    def refresh_runner(self):
        runner = self.app.runner
        if not self.settings["runner_enabled"]:
            subtitle = ("Install the Amp CLI to use this computer as a runner." if not find_cli()
                        else "Run threads from ampcode.com here with your Secrets & Env Vars.")
        else:
            subtitle = runner.summary()
            missing = runner.missing_features()
            if missing:
                subtitle += f" This Amp CLI does not support {', '.join(missing)}; update it in Updates."
        self.runner_status.set_subtitle(GLib.markup_escape_text(subtitle))
        main = self.app.main_window()
        self.network_group.set_visible(bool(main and main.app_state.get("ampNetEnabled")))
        self._refresh_runner_paths()
        if self.runner_switch.get_active() != self.settings["runner_enabled"]:
            self.runner_switch.set_active(self.settings["runner_enabled"])

    def _refresh_runner_paths(self):
        discover = self.app.runner.get("runner_discover_dirs") or []
        self.discover_row.set_subtitle(GLib.markup_escape_text(", ".join(discover) if discover else "Off"))
        new_dirs = self.app.runner.get("runner_new_dirs")
        self.new_dirs_row.set_subtitle(GLib.markup_escape_text(new_dirs or "Not set"))

    def _choose_folder(self, _button, key):
        dialog = Gtk.FileChooserNative(title="Choose Folder", transient_for=self,
                                       action=Gtk.FileChooserAction.SELECT_FOLDER, accept_label="Choose")

        def done(d, response):
            if response == Gtk.ResponseType.ACCEPT and d.get_file() and d.get_file().get_path():
                path = d.get_file().get_path()
                if key == "runner_discover_dirs":
                    current = self.app.runner.get(key) or []
                    self._runner_setting(key, current if path in current else [*current, path])
                else:
                    self._runner_setting(key, path)
                self._refresh_runner_paths()
            d.destroy()

        dialog.connect("response", done)
        dialog.show()
        self._chooser = dialog

    def _refresh_dirs(self):
        for row in self.dir_rows:
            self.dirs_group.remove(row)
        self.dir_rows = []
        for directory in self.settings["runner_dirs"]:
            row = Adw.ActionRow(title=Path(directory).name, subtitle=directory)
            row.add_prefix(Gtk.Image.new_from_icon_name("folder-symbolic"))
            remove = Gtk.Button(icon_name="user-trash-symbolic", valign=Gtk.Align.CENTER, css_classes=["flat"])
            remove.connect("clicked", lambda _b, d: self._remove_directory(d), directory)
            row.add_suffix(remove)
            self.dirs_group.add(row)
            self.dir_rows.append(row)
        if not self.dir_rows:
            row = Adw.ActionRow(title="No directories yet", subtitle="Add a folder with the + button.")
            row.add_css_class("dim-label")
            self.dirs_group.add(row)
            self.dir_rows.append(row)

    def _add_directory(self, _button):
        dialog = Gtk.FileChooserNative(title="Add Directory", transient_for=self,
                                       action=Gtk.FileChooserAction.SELECT_FOLDER, accept_label="Add")

        def done(d, response):
            if response == Gtk.ResponseType.ACCEPT and d.get_file():
                path = d.get_file().get_path()
                if path and path not in self.settings["runner_dirs"]:
                    self.settings.set("runner_dirs", [*self.settings["runner_dirs"], path])
                    self.app.runner.dirs_command("add", path)
                    self._refresh_dirs()
            d.destroy()

        dialog.connect("response", done)
        dialog.show()
        self._chooser = dialog

    def _remove_directory(self, directory):
        self.settings.set("runner_dirs", [d for d in self.settings["runner_dirs"] if d != directory])
        self.app.runner.dirs_command("remove", directory)
        self._refresh_dirs()

    # ---- Notifications -----------------------------------------------------------------------

    def _build_notifications(self):
        page = new_page()
        g = group()
        row = Adw.ActionRow(title="Notifications", subtitle="Let Amp notify you when a thread needs you.")
        self.notification_status = Gtk.Label(valign=Gtk.Align.CENTER)
        self.notification_button = Gtk.Button(label="Enable Notifications", valign=Gtk.Align.CENTER)
        self.notification_button.connect("clicked", lambda *_: self.app.enable_notifications())
        row.add_suffix(self.notification_status)
        row.add_suffix(self.notification_button)
        g.add(row)
        page.append(g)
        g2 = group(description="Notifications for the thread you are viewing are never shown. "
                               "Your choices apply to every device signed in to your account.")
        choose = Adw.ActionRow(title="Choose Notifications…",
                               subtitle="Pick which notifications Amp sends, for all your devices.", activatable=True)
        choose.add_suffix(Gtk.Image.new_from_icon_name("go-next-symbolic"))
        choose.connect("activated", lambda *_: (self.app.present_main(), self.app.main_window().app_command("openSettings")))
        g2.add(choose)
        page.append(g2)
        self.refresh_notifications()
        return page

    def refresh_notifications(self):
        allowed = self.settings["notifications"] == "authorized"
        self.notification_status.set_label("Allowed" if allowed else "Not Set Up")
        for cls in ("success", "warning"):
            self.notification_status.remove_css_class(cls)
        self.notification_status.add_css_class("success" if allowed else "warning")
        self.notification_button.set_visible(not allowed)

    # ---- Updates -----------------------------------------------------------------------------

    def _build_updates(self):
        page = new_page()
        g = group()
        web = Adw.ActionRow(title="Amp Web App", subtitle="Loaded from ampcode.com, so it is always the latest version.")
        web.add_suffix(Gtk.Label(label="Up to Date", css_classes=["success"], valign=Gtk.Align.CENTER))
        g.add(web)
        shell = Adw.ActionRow(title="Linux Shell", subtitle=f"Version {config.VERSION}. Update it with your package or the install script.")
        g.add(shell)
        page.append(g)
        g2 = group()
        cli = Adw.ActionRow(title="Amp CLI", subtitle=self._cli_version())
        button = Gtk.Button(label="Update CLI", valign=Gtk.Align.CENTER)
        button.connect("clicked", self._update_cli, cli)
        button.set_sensitive(find_cli() is not None)
        cli.add_suffix(button)
        g2.add(cli)
        page.append(g2)
        return page

    def _cli_version(self):
        cli = find_cli()
        if not cli:
            return "Not installed. Runners and thread imports need it."
        try:
            out = subprocess.run([cli, "--version"], capture_output=True, text=True, timeout=10).stdout.strip()
            return f"{out.splitlines()[0] if out else 'Installed'} · {cli}"
        except (OSError, subprocess.TimeoutExpired):
            return cli

    def _update_cli(self, button, row):
        button.set_sensitive(False)
        row.set_subtitle("Updating…")
        process = subprocess.Popen([find_cli(), "update"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        def done(_pid, _status):
            row.set_subtitle(self._cli_version())
            button.set_sensitive(True)

        GLib.child_watch_add(GLib.PRIORITY_DEFAULT, process.pid, done)

    # ---- About -------------------------------------------------------------------------------

    def _build_about(self):
        page = new_page()
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, css_classes=["card"])
        top = Gtk.Box(spacing=36, margin_top=36, margin_start=44, margin_end=44, margin_bottom=24)
        icon_path = config.icon_file(self.settings["app_icon"]) or config.icon_file("app")
        if icon_path:
            picture = Gtk.Picture.new_for_filename(str(icon_path))
            picture.set_size_request(104, 104)
            picture.set_can_shrink(True)
            top.append(picture)
        info = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4, valign=Gtk.Align.CENTER)
        wordmark = Gtk.Label(label="amp", xalign=0)
        wordmark.add_css_class("amp-wordmark")
        info.append(wordmark)
        version = Gtk.Label(label=f"{config.VERSION} (Linux)", xalign=0)
        version.add_css_class("title-3")
        version.add_css_class("dim-label")
        info.append(version)
        commit = self._git_commit()
        built = Gtk.Label(label=f"WebKitGTK {self._webkit()} · {commit}", xalign=0, css_classes=["dim-label"])
        info.append(built)
        copy = Gtk.Button(label="Copy Build Info", halign=Gtk.Align.START, margin_top=10)
        copy.connect("clicked", lambda *_: Gdk.Display.get_default().get_clipboard().set(
            f"Amp for Linux {config.VERSION} ({commit}), WebKitGTK {self._webkit()}"))
        info.append(copy)
        note = Gtk.Label(label="Amp for Linux. The app loads ampcode.com, so features arrive without updates.", xalign=0, margin_top=10,
                         css_classes=["dim-label", "caption"])
        info.append(note)
        top.append(info)
        box.append(top)
        box.append(Gtk.Separator())
        buttons = Gtk.Box(spacing=8, margin_top=16, margin_bottom=16, margin_start=20, margin_end=20)
        for label, url in (("Release Notes", "https://ampcode.com/news"), ("Documentation", "https://ampcode.com/manual")):
            b = Gtk.Button(label=label)
            b.connect("clicked", lambda _b, u: self.app.launch_uri(u), url)
            buttons.append(b)
        site = Gtk.Button(label="Open ampcode.com", hexpand=True, halign=Gtk.Align.END)
        site.connect("clicked", lambda *_: self.app.launch_uri("https://ampcode.com"))
        buttons.append(site)
        box.append(buttons)
        page.append(box)
        return page

    def _webkit(self):
        from gi.repository import WebKit
        return f"{WebKit.get_major_version()}.{WebKit.get_minor_version()}.{WebKit.get_micro_version()}"

    def _git_commit(self):
        try:
            return subprocess.run(["git", "-C", str(Path(__file__).parent), "rev-parse", "--short", "HEAD"],
                                  capture_output=True, text=True, timeout=5).stdout.strip() or "dev"
        except OSError:
            return "dev"
