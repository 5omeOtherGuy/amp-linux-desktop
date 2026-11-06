//! Settings → Runner → Directories → + → Choose Projects…: the Mac runner settings' sheet that
//! lists the Git projects other coding agents worked in, and those in folders the user adds, to
//! tick the ones threads may run in.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use serde_json::Value;

use crate::app::App;
use crate::local_projects;

/// A project found on this computer.
#[derive(Clone)]
struct Project {
    name: String,
    path: String,
    sources: Vec<String>,
}

/// Which projects the list shows.
#[derive(Clone, Copy, PartialEq)]
enum View {
    All,
    Added,
    Location(usize),
}

struct Location {
    title: String,
    projects: Vec<Project>,
}

struct Sheet {
    app: std::rc::Weak<App>,
    window: adw::Window,
    sidebar: gtk::ListBox,
    list: gtk::ListBox,
    stack: gtk::Stack,
    select_all: gtk::CheckButton,
    filter: gtk::SearchEntry,
    add: gtk::Button,
    /// Folders threads can run in already, when the sheet opened.
    added: BTreeSet<String>,
    /// Ticked projects; the added ones stay ticked.
    ticked: RefCell<BTreeSet<String>>,
    /// Recently Used (other agents' projects) first, then each folder added here.
    locations: RefCell<Vec<Location>>,
    view: Cell<View>,
    scanning: Cell<u32>,
    /// Keeps the row handlers from reacting to the ticks the list sets itself.
    rebuilding: Cell<bool>,
    #[allow(deprecated)]
    chooser: RefCell<Option<gtk::FileChooserNative>>,
}

fn agent_name(source: &str) -> &str {
    match source {
        "claude-code" => "Claude Code",
        "codex" => "Codex",
        "cursor" => "Cursor",
        "opencode" => "OpenCode",
        other => other,
    }
}

fn projects(result: &Value) -> Vec<Project> {
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    result
        .get("repositories")
        .and_then(Value::as_array)
        .map(|repos| {
            repos
                .iter()
                .map(|r| Project {
                    name: text(r, "name"),
                    path: text(r, "path"),
                    sources: amp_common::config::strings(r.get("sources").unwrap_or(&Value::Null)),
                })
                .filter(|p| !p.path.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn home_relative(path: &str) -> String {
    let home = glib::home_dir();
    match Path::new(path).strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_string(),
    }
}

/// Opens the sheet over `parent`.
pub fn show(app: &Rc<App>, parent: &impl IsA<gtk::Window>) {
    let window = adw::Window::builder()
        .application(&app.adw)
        .title("Choose Projects")
        .transient_for(parent)
        .modal(true)
        .default_width(760)
        .default_height(540)
        .build();
    let sheet = Rc::new(Sheet {
        app: Rc::downgrade(app),
        window,
        sidebar: gtk::ListBox::new(),
        list: gtk::ListBox::new(),
        stack: gtk::Stack::new(),
        select_all: gtk::CheckButton::with_label("Select All"),
        filter: gtk::SearchEntry::builder().placeholder_text("Filter").hexpand(true).build(),
        add: gtk::Button::with_label("Add Projects"),
        added: app.settings.strings("runner_dirs").into_iter().collect(),
        ticked: RefCell::default(),
        locations: RefCell::new(vec![Location { title: "Recently Used".into(), projects: Vec::new() }]),
        view: Cell::new(View::All),
        scanning: Cell::new(0),
        rebuilding: Cell::new(false),
        chooser: RefCell::default(),
    });
    sheet.build();
    sheet.window.present();
    let weak = Rc::downgrade(&sheet);
    sheet.scan(local_projects::discover, move |found| {
        if let Some(sheet) = weak.upgrade() {
            log::info!("known-agent-projects discovered {} project folder(s) from other agents", found.len());
            sheet.locations.borrow_mut()[0].projects = found;
        }
    });
    // The window keeps the sheet alive until it closes.
    let keep = RefCell::new(Some(sheet.clone()));
    sheet.window.connect_close_request(move |_| {
        keep.borrow_mut().take();
        glib::Propagation::Proceed
    });
}

impl Sheet {
    fn build(self: &Rc<Self>) {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = adw::HeaderBar::builder().show_end_title_buttons(false).show_start_title_buttons(false).build();
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let title = gtk::Label::builder().label("Choose Projects").xalign(0.0).build();
        title.add_css_class("heading");
        titles.append(&title);
        let subtitle = gtk::Label::builder()
            .label("Tick the projects threads can run in on this computer. Add a folder to find the Git projects inside it.")
            .xalign(0.0)
            .wrap(true)
            .build();
        subtitle.add_css_class("dim-label");
        subtitle.add_css_class("caption");
        titles.append(&subtitle);
        titles.set_margin_start(6);
        header.set_title_widget(Some(&gtk::Label::new(None)));
        header.pack_start(&titles);
        content.append(&header);

        let body = gtk::Paned::new(gtk::Orientation::Horizontal);
        body.set_vexpand(true);
        body.set_shrink_start_child(false);
        body.set_resize_start_child(false);
        self.sidebar.add_css_class("navigation-sidebar");
        self.sidebar.set_size_request(200, -1);
        let weak = Rc::downgrade(self);
        self.sidebar.connect_row_activated(move |_, row| {
            if let Some(this) = weak.upgrade() {
                this.sidebar_activated(row);
            }
        });
        let side_scroll =
            gtk::ScrolledWindow::builder().child(&self.sidebar).hscrollbar_policy(gtk::PolicyType::Never).build();
        body.set_start_child(Some(&side_scroll));

        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let bar =
            gtk::Box::builder().spacing(12).margin_top(8).margin_bottom(8).margin_start(12).margin_end(12).build();
        let weak = Rc::downgrade(self);
        self.select_all.connect_toggled(move |button| {
            if let Some(this) = weak.upgrade()
                && !this.rebuilding.get()
            {
                this.select_all(button.is_active());
            }
        });
        bar.append(&self.select_all);
        let weak = Rc::downgrade(self);
        self.filter.connect_search_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.refresh();
            }
        });
        bar.append(&self.filter);
        main.append(&bar);
        self.list.add_css_class("boxed-list");
        self.list.set_selection_mode(gtk::SelectionMode::None);
        self.list.set_valign(gtk::Align::Start);
        self.list.set_margin_start(12);
        self.list.set_margin_end(12);
        self.list.set_margin_bottom(12);
        let scroll = gtk::ScrolledWindow::builder().child(&self.list).vexpand(true).build();
        self.stack.add_named(&scroll, Some("list"));
        let empty = adw::StatusPage::builder()
            .icon_name("folder-new-symbolic")
            .title("No Projects Found")
            .description("Use Add Folder… to look for Git projects in a folder like ~/code.")
            .build();
        empty.add_css_class("compact");
        self.stack.add_named(&empty, Some("empty"));
        let spinner = gtk::Spinner::builder().spinning(true).width_request(32).height_request(32).build();
        let looking = adw::StatusPage::builder().title("Looking for Projects…").child(&spinner).build();
        looking.add_css_class("compact");
        self.stack.add_named(&looking, Some("scanning"));
        self.stack.set_vexpand(true);
        main.append(&self.stack);
        body.set_end_child(Some(&main));
        content.append(&body);

        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let buttons = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::End)
            .margin_top(10)
            .margin_bottom(10)
            .margin_end(12)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        let window = self.window.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(window) = window.upgrade() {
                window.close();
            }
        });
        buttons.append(&cancel);
        self.add.add_css_class("suggested-action");
        let weak = Rc::downgrade(self);
        self.add.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.add_projects();
            }
        });
        buttons.append(&self.add);
        content.append(&buttons);
        self.window.set_content(Some(&content));
        self.refresh();
    }

    /// Runs `work` off the main thread and hands its projects to `done`, showing the spinner
    /// meanwhile.
    fn scan(self: &Rc<Self>, work: impl FnOnce() -> Value + Send + 'static, done: impl FnOnce(Vec<Project>) + 'static) {
        self.scanning.set(self.scanning.get() + 1);
        self.refresh();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = local_projects::in_background(work).await;
            done(projects(&result));
            if let Some(this) = weak.upgrade() {
                this.scanning.set(this.scanning.get() - 1);
                this.refresh();
            }
        });
    }

    /// Every project the sheet knows, each once: the added folders, then those found.
    fn all(&self) -> Vec<Project> {
        let mut seen = BTreeSet::new();
        let mut all = Vec::new();
        let added = self.added.iter().map(|path| Project {
            name: Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            path: path.clone(),
            sources: Vec::new(),
        });
        let found: Vec<Project> = self.locations.borrow().iter().flat_map(|l| l.projects.clone()).collect();
        for project in added.chain(found) {
            if seen.insert(project.path.clone()) {
                all.push(project);
            }
        }
        all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.path.cmp(&b.path)));
        all
    }

    fn shown(&self) -> Vec<Project> {
        let all = self.all();
        let projects: Vec<Project> = match self.view.get() {
            View::All => all,
            View::Added => all.into_iter().filter(|p| self.added.contains(&p.path)).collect(),
            View::Location(i) => {
                let paths: BTreeSet<String> = self
                    .locations
                    .borrow()
                    .get(i)
                    .map(|l| l.projects.iter().map(|p| p.path.clone()).collect())
                    .unwrap_or_default();
                all.into_iter().filter(|p| paths.contains(&p.path)).collect()
            }
        };
        let filter = self.filter.text().to_lowercase();
        projects
            .into_iter()
            .filter(|p| {
                filter.is_empty() || p.name.to_lowercase().contains(&filter) || p.path.to_lowercase().contains(&filter)
            })
            .collect()
    }

    fn sidebar_row(&self, icon: &str, title: &str, count: Option<usize>) -> gtk::ListBoxRow {
        let line = gtk::Box::builder().spacing(10).margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).build();
        line.append(&gtk::Image::from_icon_name(icon));
        line.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build(),
        );
        if let Some(count) = count {
            let label = gtk::Label::new(Some(&count.to_string()));
            label.add_css_class("dim-label");
            line.append(&label);
        }
        gtk::ListBoxRow::builder().child(&line).build()
    }

    /// Rebuilds the sidebar counts, the list, and the buttons.
    fn refresh(self: &Rc<Self>) {
        self.rebuilding.set(true);
        while let Some(row) = self.sidebar.row_at_index(0) {
            self.sidebar.remove(&row);
        }
        let all = self.all();
        let added = all.iter().filter(|p| self.added.contains(&p.path)).count();
        let rows = [
            self.sidebar_row("view-grid-symbolic", "All Projects", Some(all.len())),
            self.sidebar_row("object-select-symbolic", "Added", Some(added)),
        ];
        for row in rows {
            self.sidebar.append(&row);
        }
        let heading = gtk::Label::builder().label("Locations").xalign(0.0).margin_top(12).margin_start(6).build();
        heading.add_css_class("dim-label");
        heading.add_css_class("caption-heading");
        let heading_row = gtk::ListBoxRow::builder().child(&heading).activatable(false).selectable(false).build();
        self.sidebar.append(&heading_row);
        for (i, location) in self.locations.borrow().iter().enumerate() {
            let icon = if i == 0 { "document-open-recent-symbolic" } else { "folder-symbolic" };
            self.sidebar.append(&self.sidebar_row(icon, &location.title, Some(location.projects.len())));
        }
        self.sidebar.append(&self.sidebar_row("list-add-symbolic", "Add Folder…", None));
        self.sidebar.select_row(self.sidebar.row_at_index(self.selected_row_index()).as_ref());

        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        let shown = self.shown();
        let ticked = self.ticked.borrow().clone();
        for project in &shown {
            let is_added = self.added.contains(&project.path);
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&project.name))
                .subtitle(glib::markup_escape_text(&{
                    let mut line = home_relative(&project.path);
                    if is_added {
                        line += " · Added";
                    } else if !project.sources.is_empty() {
                        let names: Vec<&str> = project.sources.iter().map(|s| agent_name(s)).collect();
                        line += &format!(" · {}", names.join(", "));
                    }
                    line
                }))
                .build();
            let check = gtk::CheckButton::builder()
                .active(is_added || ticked.contains(&project.path))
                .sensitive(!is_added)
                .valign(gtk::Align::Center)
                .build();
            let path = project.path.clone();
            let weak = Rc::downgrade(self);
            check.connect_toggled(move |c| {
                if let Some(this) = weak.upgrade()
                    && !this.rebuilding.get()
                {
                    if c.is_active() {
                        this.ticked.borrow_mut().insert(path.clone());
                    } else {
                        this.ticked.borrow_mut().remove(&path);
                    }
                    this.refresh_controls(&this.shown());
                }
            });
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));
            self.list.append(&row);
        }
        self.refresh_controls(&shown);
        let page = if !shown.is_empty() {
            "list"
        } else if self.scanning.get() > 0 {
            "scanning"
        } else {
            "empty"
        };
        self.stack.set_visible_child_name(page);
        self.rebuilding.set(false);
    }

    /// Select All and Add Projects, which follow the ticks.
    fn refresh_controls(&self, shown: &[Project]) {
        let was = self.rebuilding.replace(true);
        let ticked = self.ticked.borrow();
        let selectable: Vec<&Project> = shown.iter().filter(|p| !self.added.contains(&p.path)).collect();
        self.select_all.set_sensitive(!selectable.is_empty());
        self.select_all.set_active(!selectable.is_empty() && selectable.iter().all(|p| ticked.contains(&p.path)));
        self.add.set_sensitive(!ticked.is_empty());
        self.rebuilding.set(was);
    }

    /// The sidebar row of the current view; row 2 is the Locations heading.
    fn selected_row_index(&self) -> i32 {
        match self.view.get() {
            View::All => 0,
            View::Added => 1,
            View::Location(i) => 3 + i as i32,
        }
    }

    fn sidebar_activated(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let index = row.index();
        let locations = self.locations.borrow().len() as i32;
        let view = match index {
            0 => View::All,
            1 => View::Added,
            i if (3..3 + locations).contains(&i) => View::Location((i - 3) as usize),
            i if i == 3 + locations => {
                self.add_folder();
                return;
            }
            _ => return,
        };
        self.view.set(view);
        self.refresh();
    }

    fn select_all(self: &Rc<Self>, on: bool) {
        let shown = self.shown();
        {
            let mut ticked = self.ticked.borrow_mut();
            for project in shown.iter().filter(|p| !self.added.contains(&p.path)) {
                if on {
                    ticked.insert(project.path.clone());
                } else {
                    ticked.remove(&project.path);
                }
            }
        }
        self.refresh();
    }

    #[allow(deprecated)] // gtk::FileDialog picks several folders only from GTK 4.14.
    fn add_folder(self: &Rc<Self>) {
        let chooser = gtk::FileChooserNative::builder()
            .title("Add a folder to find the Git projects inside it")
            .transient_for(&self.window)
            .modal(true)
            .action(gtk::FileChooserAction::SelectFolder)
            .select_multiple(true)
            .accept_label("Add")
            .build();
        let weak = Rc::downgrade(self);
        chooser.connect_response(move |dialog, response| {
            let mut folders: Vec<String> = Vec::new();
            if response == gtk::ResponseType::Accept {
                let files = dialog.files();
                for i in 0..files.n_items() {
                    if let Some(path) =
                        files.item(i).and_then(|f| f.downcast::<gtk::gio::File>().ok()).and_then(|f| f.path())
                    {
                        folders.push(path.to_string_lossy().to_string());
                    }
                }
            }
            dialog.destroy();
            let Some(this) = weak.upgrade() else { return };
            this.chooser.borrow_mut().take();
            for folder in folders {
                let index = {
                    let mut locations = this.locations.borrow_mut();
                    locations.push(Location { title: home_relative(&folder), projects: Vec::new() });
                    locations.len() - 1
                };
                this.view.set(View::Location(index));
                let weak = Rc::downgrade(&this);
                let scanned = vec![folder];
                this.scan(
                    move || local_projects::scan_folders(&scanned),
                    move |found| {
                        if let Some(this) = weak.upgrade() {
                            this.locations.borrow_mut()[index].projects = found;
                        }
                    },
                );
            }
        });
        chooser.show();
        *self.chooser.borrow_mut() = Some(chooser);
    }

    fn add_projects(&self) {
        let paths: Vec<String> = self.ticked.borrow().iter().filter(|p| !self.added.contains(*p)).cloned().collect();
        if let Some(app) = self.app.upgrade() {
            app.add_runner_dirs(&paths);
        }
        self.window.close();
    }
}
