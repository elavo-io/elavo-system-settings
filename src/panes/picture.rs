//! Change Picture. A sidebar switches between a monogram, the camera, a
//! photo the person crops, and the suggestion pictures.
//!
//! Every round picture here is a fixed-size widget that clips itself to a
//! circle. A theme or a stretching parent box cannot turn it into a rounded
//! rectangle.

use std::cell::{Cell, RefCell};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gio, glib, graphene, gsk, pango};

use crate::avatar::Avatar;
use crate::backend::accounts::{self, User};
use crate::backend::picture::{self, Crop, Placement, TONES};
use crate::i18n::tr;
use crate::widgets;

/// The edge of the monogram preview, the viewfinder, and the crop view.
const CIRCLE: i32 = 240;
const THUMB: i32 = 80;

fn rgba(hex: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::BLACK)
}

fn circle_clip(snapshot: &gtk::Snapshot, size: f32) {
    let bounds = graphene::Rect::new(0.0, 0.0, size, size);
    snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, size / 2.0));
}

mod monogram {
    use super::*;

    #[derive(Default)]
    pub struct Monogram {
        pub letter: RefCell<String>,
        pub tone: Cell<usize>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Monogram {
        const NAME: &'static str = "ElavoPictureMonogram";
        type Type = super::Monogram;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Monogram {}

    impl WidgetImpl for Monogram {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (CIRCLE, CIRCLE, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let size = widget.width().min(widget.height()) as f32;
            circle_clip(snapshot, size);
            widget.draw(snapshot, size);
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct Monogram(ObjectSubclass<monogram::Monogram>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Monogram {
    fn new() -> Self {
        let widget: Self = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::Img)
            .build();
        fixed_square(&widget, CIRCLE);
        widget
    }

    fn set_letter(&self, letter: &str) {
        self.imp().letter.replace(letter.to_owned());
        self.queue_draw();
    }

    fn set_tone(&self, index: usize) {
        self.imp().tone.set(index.min(TONES.len() - 1));
        self.queue_draw();
    }

    /// The same drawing is the preview and the saved file, so what the
    /// person sees is what is stored. Pango picks a fallback font for a
    /// letter Roboto does not have, and the letter is centered on its ink.
    fn draw(&self, snapshot: &gtk::Snapshot, size: f32) {
        let tone = TONES[self.imp().tone.get()];
        let bounds = graphene::Rect::new(0.0, 0.0, size, size);
        snapshot.append_linear_gradient(
            &bounds,
            &graphene::Point::new(0.0, 0.0),
            &graphene::Point::new(0.0, size),
            &[
                gsk::ColorStop::new(0.0, rgba(tone.top)),
                gsk::ColorStop::new(1.0, rgba(tone.bottom)),
            ],
        );
        let letter = self.imp().letter.borrow();
        if letter.is_empty() {
            return;
        }
        let layout = self.create_pango_layout(Some(letter.as_str()));
        let mut font = pango::FontDescription::new();
        font.set_family("Roboto");
        font.set_weight(pango::Weight::Bold);
        font.set_absolute_size(f64::from(size) * 0.42 * f64::from(pango::SCALE));
        layout.set_font_description(Some(&font));
        let (ink, _) = layout.pixel_extents();
        let x = (size - ink.width() as f32) / 2.0 - ink.x() as f32;
        let y = (size - ink.height() as f32) / 2.0 - ink.y() as f32;
        snapshot.save();
        snapshot.translate(&graphene::Point::new(x, y));
        snapshot.append_layout(&layout, &rgba(picture::letter_color(tone)));
        snapshot.restore();
    }

    /// A square PNG at `ICON_EDGE`. The circle is left to whoever shows it.
    fn save_png(&self, path: &Path) -> Result<(), String> {
        let edge = picture::ICON_EDGE as f32;
        let snapshot = gtk::Snapshot::new();
        self.draw(&snapshot, edge);
        let node = snapshot
            .to_node()
            .ok_or_else(|| "The monogram could not be drawn.".to_owned())?;
        let renderer = self
            .native()
            .and_then(|native| native.renderer())
            .ok_or_else(|| "The monogram could not be drawn.".to_owned())?;
        let texture =
            renderer.render_texture(&node, Some(&graphene::Rect::new(0.0, 0.0, edge, edge)));
        texture.save_to_png(path).map_err(|error| error.to_string())
    }
}

mod cropper {
    use super::*;

    #[derive(Default)]
    pub struct Cropper {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub zoom: Cell<f32>,
        pub ox: Cell<f32>,
        pub oy: Cell<f32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Cropper {
        const NAME: &'static str = "ElavoPictureCropper";
        type Type = super::Cropper;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Cropper {}

    impl WidgetImpl for Cropper {
        fn measure(&self, _: gtk::Orientation, _: i32) -> (i32, i32, i32, i32) {
            (CIRCLE, CIRCLE, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let widget = self.obj();
            let view = widget.view();
            let placed = widget.placement();
            circle_clip(snapshot, view);
            snapshot.append_texture(
                &texture,
                &graphene::Rect::new(
                    placed.ox,
                    placed.oy,
                    texture.width() as f32 * placed.scale,
                    texture.height() as f32 * placed.scale,
                ),
            );
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct Cropper(ObjectSubclass<cropper::Cropper>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Cropper {
    fn new() -> Self {
        let widget: Self = glib::Object::builder().build();
        widget.imp().zoom.set(1.0);
        fixed_square(&widget, CIRCLE);
        widget.add_css_class("picture-circle");
        widget
    }

    fn set_texture(&self, texture: Option<gdk::Texture>) {
        let imp = self.imp();
        imp.zoom.set(1.0);
        if let Some(texture) = &texture {
            let placed = picture::centered(
                texture.width() as f32,
                texture.height() as f32,
                self.view(),
                1.0,
            );
            imp.ox.set(placed.ox);
            imp.oy.set(placed.oy);
        }
        self.set_cursor_from_name(texture.as_ref().map(|_| "grab"));
        imp.texture.replace(texture);
        self.queue_draw();
    }

    fn has_texture(&self) -> bool {
        self.imp().texture.borrow().is_some()
    }

    fn set_zoom(&self, zoom: f32) {
        let Some(texture) = self.imp().texture.borrow().clone() else {
            return;
        };
        let next = picture::zoom_about_center(
            texture.width() as f32,
            texture.height() as f32,
            self.view(),
            self.placement(),
            zoom,
        );
        self.imp().zoom.set(zoom.clamp(1.0, 3.0));
        self.imp().ox.set(next.ox);
        self.imp().oy.set(next.oy);
        self.queue_draw();
    }

    fn set_offset(&self, ox: f32, oy: f32) {
        let Some(texture) = self.imp().texture.borrow().clone() else {
            return;
        };
        let placed = picture::place(
            texture.width() as f32,
            texture.height() as f32,
            self.view(),
            self.imp().zoom.get(),
            ox,
            oy,
        );
        self.imp().ox.set(placed.ox);
        self.imp().oy.set(placed.oy);
        self.queue_draw();
    }

    fn crop(&self) -> Option<Crop> {
        let texture = self.imp().texture.borrow().clone()?;
        Some(picture::crop_square(
            texture.width() as f32,
            texture.height() as f32,
            self.view(),
            self.placement(),
        ))
    }

    fn placement(&self) -> Placement {
        let (ox, oy) = (self.imp().ox.get(), self.imp().oy.get());
        let Some(texture) = self.imp().texture.borrow().clone() else {
            return Placement { scale: 1.0, ox, oy };
        };
        picture::place(
            texture.width() as f32,
            texture.height() as f32,
            self.view(),
            self.imp().zoom.get(),
            ox,
            oy,
        )
    }

    fn view(&self) -> f32 {
        let size = self.width().min(self.height());
        if size > 1 {
            size as f32
        } else {
            CIRCLE as f32
        }
    }
}

/// Keep a widget at exactly `size` by `size` wherever it is packed.
fn fixed_square(widget: &impl IsA<gtk::Widget>, size: i32) {
    widget.set_size_request(size, size);
    widget.set_halign(gtk::Align::Center);
    widget.set_valign(gtk::Align::Center);
    widget.set_hexpand(false);
    widget.set_vexpand(false);
}

type Done = Rc<dyn Fn(&gtk::Widget, Result<(), glib::Error>)>;

struct Picker {
    window: gtk::Window,
    conn: gio::DBusConnection,
    user: RefCell<User>,
    on_done: RefCell<Option<Done>>,
    stack: gtk::Stack,
    list: gtk::ListBox,
    camera_row: gtk::ListBoxRow,
    done: gtk::Button,
    monogram: Monogram,
    swatches: Vec<gtk::ToggleButton>,
    name: gtk::Label,
    tone: Cell<usize>,
    cropper: Cropper,
    zoom: gtk::Scale,
    choose: gtk::Button,
    photo_note: gtk::Label,
    photo: RefCell<Option<PathBuf>>,
    flow: gtk::FlowBox,
    suggestions: Vec<PathBuf>,
    viewfinder: Avatar,
    camera_note: gtk::Label,
    take: gtk::Button,
    frame: RefCell<Option<Vec<u8>>>,
    captured: Cell<bool>,
    camera: RefCell<Option<std::process::Child>>,
    camera_gen: Cell<u32>,
    busy: Cell<bool>,
}

pub fn open(
    parent: &gtk::Widget,
    conn: gio::DBusConnection,
    user: User,
    on_done: impl Fn(&gtk::Widget, Result<(), glib::Error>) + 'static,
) {
    let picker = picker(parent, conn);
    if let Some(parent) = parent.root().and_downcast::<gtk::Window>() {
        picker.window.set_transient_for(Some(&parent));
    }
    let same_user = picker.user.borrow().path == user.path;
    if !same_user {
        picker.reset();
    }
    picker.monogram.set_letter(&user.initial());
    picker.name.set_label(user.display_name());
    picker.user.replace(user);
    *picker.on_done.borrow_mut() = Some(Rc::new(on_done));
    let has_camera = !picture::capture_devices().is_empty();
    picker.camera_row.set_visible(has_camera);
    if picker.page() == "camera" {
        if has_camera {
            picker.start_camera();
        } else {
            picker.list.select_row(picker.list.row_at_index(0).as_ref());
        }
    }
    picker.update_done();
    // The sidebar selects whichever row takes focus. Without this the
    // window's first focus can pick another row and switch the page.
    GtkWindowExt::set_focus(&picker.window, picker.list.selected_row().as_ref());
    picker.window.present();
}

fn picker(parent: &gtk::Widget, conn: gio::DBusConnection) -> Rc<Picker> {
    thread_local! {
        static OPEN: RefCell<Option<Rc<Picker>>> = const { RefCell::new(None) };
    }
    if let Some(existing) = OPEN.with(|slot| slot.borrow().clone()) {
        return existing;
    }
    let built = build(parent, conn);
    OPEN.with(|slot| slot.replace(Some(built.clone())));
    built
}

fn build(parent: &gtk::Widget, conn: gio::DBusConnection) -> Rc<Picker> {
    install_tones();
    let window = widgets::popup_window(parent, &tr("Change Picture"));
    window.set_default_width(760);
    window.set_default_height(600);

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["navigation-sidebar"])
        .build();
    let monogram_row = nav_row(
        "monogram",
        "avatar-default-symbolic",
        "indigo",
        &tr("Monogram"),
    );
    let camera_row = nav_row("camera", "camera-web-symbolic", "graphite", &tr("Camera"));
    let photo_row = nav_row("photo", "image-x-generic-symbolic", "teal", &tr("Photo"));
    let suggestions_row = nav_row(
        "suggestions",
        "folder-pictures-symbolic",
        "pink",
        &tr("Suggestions"),
    );
    list.append(&monogram_row);
    list.append(&camera_row);
    list.append(&photo_row);
    list.append(&suggestions_row);
    let side = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .width_request(200)
        .css_classes(["settings-sidebar"])
        .build();
    side.append(&list);

    let monogram = Monogram::new();
    monogram.update_property(&[gtk::accessible::Property::Label(&tr("Monogram"))]);
    let name = gtk::Label::builder()
        .css_classes(["dim-label"])
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(36)
        .build();
    let swatch_row = gtk::Box::builder()
        .spacing(10)
        .halign(gtk::Align::Center)
        .build();
    let mut swatches = Vec::new();
    for (index, tone) in TONES.iter().enumerate() {
        let swatch = gtk::ToggleButton::builder()
            .css_classes(["picture-swatch", &format!("picture-tone-{index}")])
            .tooltip_text(tr(tone.name))
            .build();
        fixed_square(&swatch, 28);
        if let Some(first) = swatches.first() {
            swatch.set_group(Some(first));
        } else {
            swatch.set_active(true);
        }
        swatch_row.append(&swatch);
        swatches.push(swatch);
    }
    let monogram_page = page();
    monogram_page.append(&monogram);
    monogram_page.append(&swatch_row);
    monogram_page.append(&name);

    let viewfinder = Avatar::empty(CIRCLE);
    viewfinder.add_css_class("picture-circle");
    let take = gtk::Button::builder()
        .label(tr("Take Picture"))
        .halign(gtk::Align::Center)
        .sensitive(false)
        .build();
    let camera_note = note("");
    let camera_page = page();
    camera_page.append(&viewfinder);
    camera_page.append(&take);
    camera_page.append(&camera_note);

    let cropper = Cropper::new();
    let zoom = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 3.0, 0.05);
    zoom.set_draw_value(false);
    zoom.set_width_request(200);
    zoom.set_halign(gtk::Align::Center);
    zoom.set_visible(false);
    let zoom_row = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    let smaller = gtk::Image::from_icon_name("image-x-generic-symbolic");
    smaller.set_pixel_size(12);
    let larger = gtk::Image::from_icon_name("image-x-generic-symbolic");
    larger.set_pixel_size(18);
    zoom_row.append(&smaller);
    zoom_row.append(&zoom);
    zoom_row.append(&larger);
    zoom.bind_property("visible", &zoom_row, "visible")
        .sync_create()
        .build();
    let photo_note = note(&tr("Choose a photo, then drag it to frame the circle."));
    let choose = gtk::Button::builder()
        .label(tr("Choose a Photo…"))
        .halign(gtk::Align::Center)
        .build();
    let photo_page = page();
    photo_page.append(&cropper);
    photo_page.append(&zoom_row);
    photo_page.append(&photo_note);
    photo_page.append(&choose);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .activate_on_single_click(false)
        .min_children_per_line(3)
        .max_children_per_line(5)
        .column_spacing(8)
        .row_spacing(8)
        .homogeneous(true)
        .valign(gtk::Align::Start)
        .css_classes(["picture-grid"])
        .margin_top(20)
        .margin_bottom(20)
        .margin_start(20)
        .margin_end(20)
        .build();
    let suggestions = picture::suggestions(&picture::suggestion_roots());
    let mut thumbs = Vec::new();
    for path in &suggestions {
        let thumb = Avatar::empty(THUMB);
        thumb.add_css_class("picture-circle");
        if let Some(stem) = path.file_stem() {
            thumb.update_property(&[gtk::accessible::Property::Label(&stem.to_string_lossy())]);
        }
        flow.append(&thumb);
        thumbs.push(thumb);
    }
    let suggestions_page: gtk::Widget = if suggestions.is_empty() {
        let empty = page();
        empty.append(&note(&tr("No suggestion pictures are installed.")));
        empty.upcast()
    } else {
        gtk::ScrolledWindow::builder()
            .child(&flow)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .hexpand(true)
            .vexpand(true)
            .build()
            .upcast()
    };
    load_thumbs(suggestions.clone(), thumbs);

    let stack = gtk::Stack::builder()
        .hexpand(true)
        .vexpand(true)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&monogram_page, Some("monogram"));
    stack.add_named(&camera_page, Some("camera"));
    stack.add_named(&photo_page, Some("photo"));
    stack.add_named(&suggestions_page, Some("suggestions"));

    let done = gtk::Button::builder()
        .label(tr("Done"))
        .css_classes(["suggested-action"])
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(12)
        .margin_bottom(12)
        .margin_end(16)
        .margin_start(16)
        .build();
    buttons.append(&cancel);
    buttons.append(&done);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    content.append(&stack);
    content.append(&buttons);

    let body = gtk::Box::builder().hexpand(true).vexpand(true).build();
    body.append(&side);
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    body.append(&content);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&done));

    let escape = gtk::ShortcutController::new();
    escape.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(escape);

    let picker = Rc::new(Picker {
        window: window.clone(),
        conn,
        user: RefCell::new(User {
            path: String::new(),
            uid: 0,
            name: String::new(),
            real_name: String::new(),
            admin: false,
            icon: String::new(),
        }),
        on_done: RefCell::new(None),
        stack: stack.clone(),
        list: list.clone(),
        camera_row: camera_row.clone(),
        done: done.clone(),
        monogram,
        swatches,
        name,
        tone: Cell::new(0),
        cropper: cropper.clone(),
        zoom: zoom.clone(),
        choose: choose.clone(),
        photo_note,
        photo: RefCell::new(None),
        flow: flow.clone(),
        suggestions,
        viewfinder,
        camera_note,
        take: take.clone(),
        frame: RefCell::new(None),
        captured: Cell::new(false),
        camera: RefCell::new(None),
        camera_gen: Cell::new(0),
        busy: Cell::new(false),
    });

    for (index, swatch) in picker.swatches.iter().enumerate() {
        let this = picker.clone();
        swatch.connect_toggled(move |swatch| {
            if swatch.is_active() {
                this.tone.set(index);
                this.monogram.set_tone(index);
            }
        });
    }

    let this = picker.clone();
    list.connect_row_selected(move |_, row| {
        let Some(row) = row else { return };
        let name = row.widget_name();
        this.stack.set_visible_child_name(&name);
        if name == "camera" {
            this.start_camera();
        } else {
            this.stop_camera();
        }
        this.update_done();
    });
    list.select_row(Some(&monogram_row));

    let this = picker.clone();
    choose.connect_clicked(move |_| this.choose_photo());
    let click = gtk::GestureClick::new();
    let this = picker.clone();
    click.connect_released(move |_, _, _, _| {
        if !this.cropper.has_texture() && !this.busy.get() {
            this.choose_photo();
        }
    });
    cropper.add_controller(click);
    let this = picker.clone();
    zoom.connect_value_changed(move |scale| this.cropper.set_zoom(scale.value() as f32));
    wire_crop_gestures(&cropper, &zoom);

    let this = picker.clone();
    flow.connect_selected_children_changed(move |_| this.update_done());
    let this = picker.clone();
    flow.connect_child_activated(move |_, _| this.save());
    let this = picker.clone();
    take.connect_clicked(move |_| this.take_or_retake());
    let this = picker.clone();
    done.connect_clicked(move |_| this.save());
    let this = picker.clone();
    cancel.connect_clicked(move |_| this.window.close());
    let this = picker.clone();
    window.connect_hide(move |_| this.stop_camera());

    picker
}

impl Picker {
    fn page(&self) -> String {
        self.stack
            .visible_child_name()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "monogram".into())
    }

    /// Forget the last person's photo, suggestion, and tone.
    fn reset(&self) {
        self.photo.replace(None);
        self.cropper.set_texture(None);
        self.zoom.set_visible(false);
        self.zoom.set_value(1.0);
        self.choose.set_label(&tr("Choose a Photo…"));
        widgets::set_status(
            &self.photo_note,
            &tr("Choose a photo, then drag it to frame the circle."),
        );
        self.flow.unselect_all();
        if let Some(first) = self.swatches.first() {
            first.set_active(true);
        }
    }

    fn update_done(&self) {
        let busy = self.busy.get();
        let ready = match self.page().as_str() {
            "camera" => self.captured.get(),
            "photo" => self.photo.borrow().is_some() && self.cropper.has_texture(),
            "suggestions" => self.selected_suggestion().is_some(),
            _ => true,
        };
        self.done.set_sensitive(ready && !busy);
        self.take
            .set_sensitive(!busy && (self.captured.get() || self.frame.borrow().is_some()));
        self.choose.set_sensitive(!busy);
    }

    fn selected_suggestion(&self) -> Option<PathBuf> {
        let child = self.flow.selected_children().into_iter().next()?;
        self.suggestions.get(child.index() as usize).cloned()
    }

    fn choose_photo(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let images = gtk::FileFilter::new();
            images.set_name(Some(&tr("Images")));
            images.add_mime_type("image/*");
            let all = gtk::FileFilter::new();
            all.set_name(Some(&tr("All Files")));
            all.add_pattern("*");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&images);
            filters.append(&all);
            let dialog = gtk::FileDialog::builder()
                .title(tr("Choose a Photo"))
                .filters(&filters)
                .modal(true)
                .build();
            let Ok(file) = dialog.open_future(Some(&this.window)).await else {
                return;
            };
            let Some(input) = file.path() else {
                widgets::show_error(
                    &this.window,
                    &tr("That picture could not be opened."),
                    &tr("Only pictures on this computer can be used."),
                );
                return;
            };
            this.busy.set(true);
            this.update_done();
            widgets::set_status(&this.photo_note, &tr("Opening the photo…"));
            let working = scratch(&format!("photo-source-{}.jpg", this.user.borrow().uid));
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let loaded = picture::normalize(&input, &working).and_then(|()| {
                    gdk::Texture::from_filename(&working)
                        .map(|texture| (working, texture))
                        .map_err(|error| error.to_string())
                });
                let _ = tx.send_blocking(loaded);
            });
            let loaded = rx
                .recv()
                .await
                .unwrap_or_else(|_| Err(tr("That picture could not be opened.")));
            this.busy.set(false);
            match loaded {
                Ok((working, texture)) => {
                    this.cropper.set_texture(Some(texture));
                    this.zoom.set_value(1.0);
                    this.zoom.set_visible(true);
                    this.photo.replace(Some(working));
                    this.choose.set_label(&tr("Choose Another Photo…"));
                    widgets::set_status(
                        &this.photo_note,
                        &tr("Drag the photo to move it. The circle is the part that is saved."),
                    );
                }
                Err(detail) => {
                    widgets::set_status(
                        &this.photo_note,
                        &if this.cropper.has_texture() {
                            tr("Drag the photo to move it. The circle is the part that is saved.")
                        } else {
                            tr("Choose a photo, then drag it to frame the circle.")
                        },
                    );
                    widgets::show_error(
                        &this.window,
                        &tr("That picture could not be opened."),
                        &detail,
                    );
                }
            }
            this.update_done();
        });
    }

    fn save(self: &Rc<Self>) {
        if self.busy.get() || !self.done.is_sensitive() {
            return;
        }
        match self.page().as_str() {
            "camera" => self.save_camera(),
            "photo" => self.save_photo(),
            "suggestions" => self.save_suggestion(),
            _ => self.save_monogram(),
        }
    }

    fn save_monogram(self: &Rc<Self>) {
        let output = scratch(&format!("monogram-{}.png", self.user.borrow().uid));
        match self.monogram.save_png(&output) {
            Ok(()) => self.run_prepare(move || Ok(output)),
            Err(detail) => widgets::show_error(
                &self.window,
                &tr("The picture could not be prepared."),
                &detail,
            ),
        }
    }

    fn save_photo(self: &Rc<Self>) {
        let Some(input) = self.photo.borrow().clone() else {
            return;
        };
        let Some(crop) = self.cropper.crop() else {
            return;
        };
        let output = scratch(&format!("photo-{}.jpg", self.user.borrow().uid));
        self.run_prepare(move || {
            picture::render_crop(&input, crop, &output)?;
            Ok(output)
        });
    }

    fn save_camera(self: &Rc<Self>) {
        let Some(frame) = self.frame.borrow().clone() else {
            return;
        };
        let uid = self.user.borrow().uid;
        let raw = scratch(&format!("camera-raw-{uid}.jpg"));
        let output = scratch(&format!("camera-{uid}.jpg"));
        self.run_prepare(move || {
            std::fs::write(&raw, &frame).map_err(|error| error.to_string())?;
            picture::render_square(&raw, &output)?;
            Ok(output)
        });
    }

    fn save_suggestion(self: &Rc<Self>) {
        let Some(path) = self.selected_suggestion() else {
            return;
        };
        let output = scratch(&format!("suggestion-{}.jpg", self.user.borrow().uid));
        self.run_prepare(move || picture::prepare_suggestion(&path, &output));
    }

    /// The account is fixed when the person presses Done. The window can be
    /// reopened for someone else while ImageMagick is still working.
    fn run_prepare(
        self: &Rc<Self>,
        prepare: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
    ) {
        let account = self.user.borrow().path.clone();
        let on_done = self.on_done.borrow().clone();
        self.busy.set(true);
        self.update_done();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(prepare());
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            let prepared = rx
                .recv()
                .await
                .unwrap_or_else(|_| Err(tr("The picture could not be prepared.")));
            let result = match prepared {
                Ok(file) => accounts::set_icon(&this.conn, &account, &file.to_string_lossy()).await,
                Err(detail) => {
                    widgets::show_error(
                        &this.window,
                        &tr("The picture could not be prepared."),
                        &detail,
                    );
                    this.busy.set(false);
                    this.update_done();
                    return;
                }
            };
            this.busy.set(false);
            if result.is_ok() {
                this.window.close();
            }
            this.update_done();
            if let Some(on_done) = on_done {
                on_done(this.window.upcast_ref(), result);
            }
        });
    }

    fn take_or_retake(self: &Rc<Self>) {
        if self.captured.get() {
            self.start_camera();
            return;
        }
        if self.frame.borrow().is_none() {
            return;
        }
        self.end_camera_process();
        self.captured.set(true);
        self.take.set_label(&tr("Retake"));
        widgets::set_status(&self.camera_note, &tr("Press Done to use this picture."));
        self.update_done();
    }

    fn start_camera(self: &Rc<Self>) {
        self.stop_camera();
        let Some(device) = picture::capture_devices().into_iter().next() else {
            widgets::set_status(&self.camera_note, &tr("No camera is connected."));
            return;
        };
        let mut child = match Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-fflags",
                "nobuffer",
                "-f",
                "v4l2",
                "-i",
            ])
            .arg(&device)
            .args([
                "-an",
                "-vf",
                // Mirrored, like looking in a mirror. The saved picture is
                // the frame the person saw.
                "hflip,scale=480:480:force_original_aspect_ratio=increase,crop=480:480",
                "-r",
                "15",
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "-q:v",
                "4",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                widgets::set_status(&self.camera_note, &tr("The camera could not be opened."));
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        self.camera.replace(Some(child));
        widgets::set_status(&self.camera_note, &tr("Starting the camera…"));
        let gen = self.camera_gen.get();
        let (tx, rx) = async_channel::bounded::<Vec<u8>>(1);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Ok(frame) = picture::next_jpeg(&mut reader) {
                if tx.send_blocking(frame).is_err() {
                    break;
                }
            }
        });
        let this = self.clone();
        glib::spawn_future_local(async move {
            let mut saw_frame = false;
            while let Ok(jpeg) = rx.recv().await {
                if this.camera_gen.get() != gen {
                    return;
                }
                let bytes = glib::Bytes::from(&jpeg);
                if let Ok(texture) = gdk::Texture::from_bytes(&bytes) {
                    this.viewfinder.set_texture(Some(texture));
                    this.frame.replace(Some(jpeg));
                    if !saw_frame {
                        saw_frame = true;
                        widgets::set_status(&this.camera_note, "");
                        this.update_done();
                    }
                }
            }
            if this.camera_gen.get() != gen {
                return;
            }
            this.end_camera_process();
            this.frame.replace(None);
            this.viewfinder.set_texture(None);
            widgets::set_status(
                &this.camera_note,
                &if saw_frame {
                    tr("The camera stopped.")
                } else {
                    tr("The camera could not be opened. Another app may be using it.")
                },
            );
            this.update_done();
        });
    }

    /// Stop ffmpeg and ignore any frame still on its way. The last frame stays.
    fn end_camera_process(&self) {
        self.camera_gen.set(self.camera_gen.get().wrapping_add(1));
        if let Some(mut child) = self.camera.borrow_mut().take() {
            let _ = child.kill();
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    fn stop_camera(&self) {
        self.end_camera_process();
        self.frame.replace(None);
        self.captured.set(false);
        self.viewfinder.set_texture(None);
        self.take.set_label(&tr("Take Picture"));
        widgets::set_status(&self.camera_note, "");
    }
}

/// Decode the suggestion pictures off the main thread. There are two dozen
/// 1024px JPEGs, too many to decode while the window opens.
fn load_thumbs(paths: Vec<PathBuf>, thumbs: Vec<Avatar>) {
    let (tx, rx) = async_channel::unbounded::<(usize, gdk::Texture)>();
    std::thread::spawn(move || {
        for (index, path) in paths.iter().enumerate() {
            if let Ok(texture) = gdk::Texture::from_filename(path) {
                if tx.send_blocking((index, texture)).is_err() {
                    break;
                }
            }
        }
    });
    glib::spawn_future_local(async move {
        while let Ok((index, texture)) = rx.recv().await {
            if let Some(thumb) = thumbs.get(index) {
                thumb.set_texture(Some(texture));
            }
        }
    });
}

fn wire_crop_gestures(cropper: &Cropper, zoom: &gtk::Scale) {
    let drag = gtk::GestureDrag::new();
    let origin = Rc::new(Cell::new((0.0_f32, 0.0_f32)));
    let widget = cropper.clone();
    let start = origin.clone();
    drag.connect_drag_begin(move |_, _, _| {
        let imp = widget.imp();
        start.set((imp.ox.get(), imp.oy.get()));
        if widget.has_texture() {
            widget.set_cursor_from_name(Some("grabbing"));
        }
    });
    let widget = cropper.clone();
    drag.connect_drag_update(move |_, dx, dy| {
        let (x, y) = origin.get();
        widget.set_offset(x + dx as f32, y + dy as f32);
    });
    let widget = cropper.clone();
    drag.connect_drag_end(move |_, _, _| {
        if widget.has_texture() {
            widget.set_cursor_from_name(Some("grab"));
        }
    });
    cropper.add_controller(drag);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let widget = cropper.clone();
    let zoom = zoom.clone();
    scroll.connect_scroll(move |_, _, dy| {
        if !widget.has_texture() {
            return glib::Propagation::Proceed;
        }
        let factor = if dy < 0.0 { 1.08 } else { 1.0 / 1.08 };
        zoom.set_value(zoom.value() * factor);
        glib::Propagation::Stop
    });
    cropper.add_controller(scroll);
}

fn nav_row(id: &str, icon: &str, color: &str, label: &str) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(8).build();
    content.append(&widgets::tile(icon, color, "small"));
    content.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::End)
            .build(),
    );
    gtk::ListBoxRow::builder().child(&content).name(id).build()
}

fn page() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .margin_top(24)
        .margin_bottom(12)
        .margin_start(24)
        .margin_end(24)
        .build()
}

fn note(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(40)
        .halign(gtk::Align::Center)
        .css_classes(["caption", "dim-label"])
        .build();
    label.set_visible(!text.is_empty());
    label
}

fn scratch(name: &str) -> PathBuf {
    let dir = glib::user_cache_dir().join("elavo");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

/// Swatch backgrounds. They are at application priority so the theme's
/// button faces and hover layers cannot replace the gradient.
fn install_tones() {
    let mut css = String::new();
    for (index, tone) in TONES.iter().enumerate() {
        css.push_str(&format!(
            ".picture-swatch.picture-tone-{index} {{ background-image: linear-gradient(to bottom, {top}, {bottom}); }}\n",
            top = tone.top,
            bottom = tone.bottom,
        ));
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        if let Some(mut child) = self.camera.borrow_mut().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
