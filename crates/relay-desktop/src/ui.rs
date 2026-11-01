use crate::{
    preferences::{Preferences, QueuedFile},
    tray::TrayAction,
    Host, Signal, SIGNALS,
};
use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, qr_code, row, scrollable, text,
    text_editor, text_input, Space,
};
use iced::{
    alignment, clipboard, event, window, Border, Color, Element, Font, Length, Point, Size,
    Subscription, Task, Theme,
};
use relay_core::{Invite, Item, ItemPage, NewFeedback, NewText, PatchItem, Snapshot, OWNER_ID};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Feed,
    Devices,
    Transfers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Composer {
    Files,
    Text,
}

#[derive(Debug, Clone, Copy)]
pub struct WorkArea {
    origin: Point,
    size: Size,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildChoice {
    id: String,
    label: String,
}
impl fmt::Display for BuildChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectFilter {
    All,
    Unassigned,
    Named(String),
}

impl fmt::Display for ProjectFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("全部项目"),
            Self::Unassigned => f.write_str("未分项目"),
            Self::Named(name) => f.write_str(name),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Signal(Signal),
    Snapshot(Result<Snapshot, String>),
    History(Result<ItemPage, String>, bool),
    Tick,
    Clock,
    Opened,
    Resized(Size),
    Rescaled,
    WorkArea(Option<WorkArea>, bool),
    CloseRequested(window::Id),
    Dropped(PathBuf),
    Page(Page),
    Composer(Composer),
    ProjectOptions,
    Project(String),
    Query(String),
    Filter(ProjectFilter),
    Editor(text_editor::Action),
    Send,
    Sent(Result<Item, String>),
    ChooseFiles,
    FilesChosen(Vec<PathBuf>),
    Imported(Result<Item, String>),
    ToggleQueue,
    RemoveQueued(usize),
    Address(String),
    Addresses(Result<Vec<String>, String>),
    NewInvite,
    Invited(Result<Invite, String>),
    Pair(String, bool),
    RevokePrompt(Option<String>),
    Revoke(String),
    CancelUpload(String),
    Operation(Result<String, String>),
    Copy(String),
    Details(Option<String>),
    Build(Option<BuildChoice>),
    Attachment(String, bool),
    Mark(String, String),
    SaveFile(String),
    Export(String),
    OpenInbox,
    More,
    Reload,
    Quit,
    Stopped,
}

struct ImportProgress {
    name: String,
    done: u64,
    total: u64,
}

struct TransferSample {
    at: Instant,
    bytes: u64,
    rate: Option<f64>,
    advanced: Instant,
}

impl TransferSample {
    fn observe(&mut self, bytes: u64, now: Instant) {
        let elapsed = now.duration_since(self.at).as_secs_f64();
        if bytes < self.bytes {
            self.rate = None;
            self.bytes = bytes;
            self.at = now;
        } else if bytes > self.bytes && elapsed >= 0.05 {
            let measured = (bytes - self.bytes) as f64 / elapsed;
            self.rate = Some(
                self.rate
                    .map_or(measured, |old| old * 0.35 + measured * 0.65),
            );
            self.advanced = now;
            self.bytes = bytes;
            self.at = now;
        }
    }

    fn rate(&self) -> Option<f64> {
        self.rate_at(Instant::now())
    }

    fn rate_at(&self, now: Instant) -> Option<f64> {
        (now.duration_since(self.advanced) < Duration::from_secs(4))
            .then_some(self.rate)
            .flatten()
    }
}

pub struct App {
    host: Arc<Host>,
    window: Option<window::Id>,
    tray_enabled: bool,
    snapshot: Option<Snapshot>,
    snapshot_loading: bool,
    refresh_pending: bool,
    items: Vec<Item>,
    next_cursor: Option<String>,
    history_loading: bool,
    page: Page,
    viewport: Size,
    composer: Composer,
    project_options: bool,
    transfer_samples: HashMap<String, TransferSample>,
    project: String,
    query: String,
    filter: ProjectFilter,
    editor: text_editor::Content,
    sending: bool,
    pending_operations: usize,
    send_request_id: Option<String>,
    feedback_build: Option<BuildChoice>,
    attachments: HashSet<String>,
    selected_item: Option<String>,
    addresses: Vec<String>,
    selected_address: Option<String>,
    invite: Option<Invite>,
    qr: Option<qr_code::Data>,
    invite_loading: bool,
    queue: VecDeque<QueuedFile>,
    importing: Option<ImportProgress>,
    queue_paused: bool,
    revoke_prompt: Option<String>,
    notice: String,
    error: bool,
    exiting: bool,
}

impl App {
    pub fn boot(
        host: Arc<Host>,
        tray_enabled: bool,
        tray_notice: Option<String>,
    ) -> (Self, Task<Message>) {
        let preferences = Preferences::load(&host.directory);
        let addresses = local_addresses(host.port);
        let selected_address = addresses
            .iter()
            .find(|a| **a == preferences.address)
            .cloned()
            .or_else(|| addresses.first().cloned());
        let queue: VecDeque<_> = preferences.pending_files.into();
        let mut notice = tray_notice.unwrap_or_default();
        if !queue.is_empty() {
            notice = "发现上次未完成的本机队列。请检查源文件后，在「传输任务」中继续。".into();
        }
        let (id, open) = window::open(window_settings());
        let mut app = Self {
            host,
            window: Some(id),
            tray_enabled,
            snapshot: None,
            snapshot_loading: false,
            refresh_pending: false,
            items: vec![],
            next_cursor: None,
            history_loading: false,
            page: Page::Feed,
            viewport: Size::new(1120.0, 760.0),
            composer: Composer::Files,
            project_options: false,
            transfer_samples: HashMap::new(),
            project: preferences.project,
            query: String::new(),
            filter: ProjectFilter::All,
            editor: text_editor::Content::new(),
            sending: false,
            pending_operations: 0,
            send_request_id: None,
            feedback_build: None,
            attachments: HashSet::new(),
            selected_item: None,
            addresses,
            selected_address,
            invite: None,
            qr: None,
            invite_loading: false,
            queue,
            importing: None,
            queue_paused: true,
            revoke_prompt: None,
            notice,
            error: false,
            exiting: false,
        };
        let snapshot = app.refresh();
        let history = app.history(true);
        (
            app,
            Task::batch([open.map(|_| Message::Opened), snapshot, history]),
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            Subscription::run(signal_stream),
            window::close_requests().map(Message::CloseRequested),
            event::listen_with(|event, _, _| match event {
                iced::Event::Window(window::Event::FileDropped(path)) => {
                    Some(Message::Dropped(path))
                }
                iced::Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
                iced::Event::Window(window::Event::Rescaled(_)) => Some(Message::Rescaled),
                _ => None,
            }),
        ];
        if self.window.is_some() && !self.exiting {
            subscriptions.push(iced::time::every(Duration::from_secs(10)).map(|_| Message::Tick));
            if self
                .transfer_samples
                .values()
                .any(|sample| sample.rate().is_some())
            {
                subscriptions
                    .push(iced::time::every(Duration::from_secs(1)).map(|_| Message::Clock));
            }
        }
        Subscription::batch(subscriptions)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if self.exiting
            && !matches!(
                &message,
                Message::Imported(_)
                    | Message::Sent(_)
                    | Message::Operation(_)
                    | Message::Invited(_)
                    | Message::Signal(Signal::ImportProgress { .. })
                    | Message::Stopped
            )
        {
            return Task::none();
        }
        match message {
            Message::Opened => self.fit_window(true),
            Message::Resized(size) => {
                self.viewport = size;
                Task::none()
            }
            Message::Rescaled => self.fit_window(false),
            Message::Clock => Task::none(),
            Message::WorkArea(area, initial) => {
                let Some(id) = self.window else {
                    return Task::none();
                };
                let area = area.unwrap_or(WorkArea {
                    origin: Point::ORIGIN,
                    size: Size::new(1024.0, 768.0),
                });
                let preferred = if initial {
                    Size::new(1120.0, 760.0)
                } else {
                    self.viewport
                };
                let size = fitted_size(preferred, area.size);
                self.viewport = size;
                let position = Point::new(
                    area.origin.x + (area.size.width - size.width) / 2.0,
                    area.origin.y + (area.size.height - size.height - 32.0).max(0.0) / 2.0,
                );
                Task::batch([
                    window::set_min_size(
                        id,
                        Some(Size::new(size.width.min(620.0), size.height.min(420.0))),
                    ),
                    window::resize(id, size),
                    window::move_to(id, position),
                ])
                .chain(window::set_mode(id, window::Mode::Windowed))
            }
            Message::Signal(Signal::Tray(TrayAction::Show)) => self.show_window(),
            Message::Signal(Signal::Tray(TrayAction::Quit)) | Message::Quit => self.quit(),
            Message::Signal(Signal::Refresh) | Message::Tick => {
                if self.window.is_none() {
                    self.refresh_pending = true;
                    Task::none()
                } else {
                    self.refresh()
                }
            }
            Message::Signal(Signal::ImportProgress { name, done, total }) => {
                if let Some(progress) = &mut self.importing {
                    progress.name = name;
                    progress.done = done;
                    progress.total = total;
                }
                Task::none()
            }
            Message::Snapshot(result) => {
                self.snapshot_loading = false;
                match result {
                    Ok(snapshot) => {
                        let now = Instant::now();
                        self.transfer_samples.retain(|id, _| {
                            snapshot
                                .uploads
                                .iter()
                                .any(|u| u.id == *id && u.status == "uploading")
                        });
                        for upload in snapshot.uploads.iter().filter(|u| u.status == "uploading") {
                            self.transfer_samples
                                .entry(upload.id.clone())
                                .and_modify(|sample| sample.observe(upload.offset, now))
                                .or_insert(TransferSample {
                                    at: now,
                                    bytes: upload.offset,
                                    rate: None,
                                    advanced: now,
                                });
                        }
                        self.merge_items(snapshot.items.clone());
                        self.snapshot = Some(snapshot);
                    }
                    Err(error) => self.fail(error),
                }
                if self.refresh_pending && !self.exiting {
                    self.refresh_pending = false;
                    self.refresh()
                } else {
                    Task::none()
                }
            }
            Message::History(result, reset) => {
                self.history_loading = false;
                match result {
                    Ok(page) => {
                        if reset {
                            self.items.clear();
                        }
                        self.merge_items(page.items);
                        self.next_cursor = page.next_cursor;
                    }
                    Err(error) => self.fail(error),
                }
                Task::none()
            }
            Message::CloseRequested(id) => {
                if self.tray_enabled && !self.exiting {
                    self.window = None;
                    window::close(id)
                } else {
                    self.quit()
                }
            }
            Message::Page(page) => {
                if self.page != page && !self.error {
                    self.notice.clear();
                }
                self.page = page;
                if page == Page::Devices && self.invite.is_none() {
                    Task::done(Message::NewInvite)
                } else {
                    Task::none()
                }
            }
            Message::Composer(composer) => {
                self.composer = composer;
                Task::none()
            }
            Message::ProjectOptions => {
                self.project_options = !self.project_options;
                Task::none()
            }
            Message::Project(project) => {
                if !self.sending && project.chars().count() <= 100 {
                    self.project = project;
                    self.send_request_id = None;
                }
                Task::none()
            }
            Message::Query(query) => {
                self.query = query;
                Task::none()
            }
            Message::Filter(filter) => {
                self.filter = filter;
                Task::none()
            }
            Message::Editor(action) => {
                if !self.sending {
                    self.editor.perform(action);
                    self.send_request_id = None;
                }
                Task::none()
            }
            Message::Send => self.send(),
            Message::Sent(result) => {
                self.sending = false;
                let task = match result {
                    Ok(item) => {
                        self.merge_items(vec![item]);
                        self.editor = text_editor::Content::new();
                        self.attachments.clear();
                        self.feedback_build = None;
                        self.send_request_id = None;
                        self.inform("已发送到共同收件区。");
                        self.refresh()
                    }
                    Err(error) => {
                        self.fail(error);
                        Task::none()
                    }
                };
                if self.ready_to_exit() {
                    self.stop_server()
                } else {
                    task
                }
            }
            Message::ChooseFiles => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title("选择原图、APK、录屏或文件")
                        .pick_files()
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|f| f.path().to_owned())
                        .collect()
                },
                Message::FilesChosen,
            ),
            Message::Dropped(path) => self.enqueue(vec![path]),
            Message::FilesChosen(paths) => self.enqueue(paths),
            Message::Imported(result) => {
                self.importing = None;
                match result {
                    Ok(item) => {
                        self.queue.pop_front();
                        self.inform(format!("{} 已导入，手机可以下载原文件。", item.name));
                        self.merge_items(vec![item]);
                    }
                    Err(error) => {
                        self.queue_paused = true;
                        self.fail(format!(
                            "导入暂停：{error}。队列与已确认断点已保留。恢复原文件后继续；若要换文件，请移出此项后重新添加。"
                        ));
                    }
                }
                self.save_preferences();
                if self.ready_to_exit() {
                    self.stop_server()
                } else if self.exiting {
                    Task::none()
                } else {
                    Task::batch([self.next_import(), self.refresh()])
                }
            }
            Message::ToggleQueue => {
                self.queue_paused = !self.queue_paused;
                if self.queue_paused && self.importing.is_some() {
                    self.inform("当前导入完成后暂停后续文件；手机的网络传输不受影响。");
                }
                self.next_import()
            }
            Message::RemoveQueued(index) => {
                if !(index == 0 && self.importing.is_some()) {
                    let removed = self.queue.remove(index);
                    self.save_preferences();
                    if let Some(queued) = removed {
                        let upload = self.snapshot.as_ref().and_then(|snapshot| {
                            snapshot.uploads.iter().find(|upload| {
                                upload.request_id == queued.request_id
                                    && upload.device_id == OWNER_ID
                                    && (upload.status == "uploading" || upload.status == "failed")
                            })
                        });
                        if let Some(upload) = upload {
                            return Task::done(Message::CancelUpload(upload.id.clone()));
                        }
                    }
                }
                Task::none()
            }
            Message::Address(address) => {
                self.selected_address = Some(address);
                self.rebuild_qr();
                self.save_preferences();
                Task::none()
            }
            Message::Addresses(result) => {
                match result {
                    Ok(addresses) => {
                        if !self
                            .selected_address
                            .as_ref()
                            .is_some_and(|selected| addresses.contains(selected))
                        {
                            self.selected_address = addresses.first().cloned();
                        }
                        self.addresses = addresses;
                        self.rebuild_qr();
                    }
                    Err(error) => self.fail(error),
                }
                Task::none()
            }
            Message::NewInvite => {
                if self.invite_loading {
                    return Task::none();
                }
                self.invite_loading = true;
                self.pending_operations += 1;
                let relay = self.host.relay.clone();
                let port = self.host.port;
                Task::batch([
                    blocking(
                        move || relay.create_invite().map_err(|e| e.to_string()),
                        Message::Invited,
                    ),
                    blocking(move || Ok(local_addresses(port)), Message::Addresses),
                ])
            }
            Message::Invited(result) => {
                self.invite_loading = false;
                self.pending_operations = self.pending_operations.saturating_sub(1);
                match result {
                    Ok(invite) => {
                        self.invite = Some(invite);
                        self.rebuild_qr();
                        self.inform("连接码已更新。扫码或输入连接码后，仍需你在电脑批准。");
                    }
                    Err(error) => self.fail(error),
                }
                if self.ready_to_exit() {
                    self.stop_server()
                } else {
                    Task::none()
                }
            }
            Message::Pair(id, approve) => {
                self.pending_operations += 1;
                let relay = self.host.relay.clone();
                blocking(
                    move || {
                        relay.decide_pair(&id, approve).map_err(|e| e.to_string())?;
                        Ok(if approve {
                            "已批准设备，请在手机等待连接完成。"
                        } else {
                            "已拒绝连接请求。"
                        }
                        .into())
                    },
                    Message::Operation,
                )
            }
            Message::RevokePrompt(id) => {
                self.revoke_prompt = id;
                Task::none()
            }
            Message::Revoke(id) => {
                self.pending_operations += 1;
                self.revoke_prompt = None;
                let relay = self.host.relay.clone();
                blocking(
                    move || {
                        relay.revoke_device(&id).map_err(|e| e.to_string())?;
                        Ok("已撤销设备，之后的请求需要重新配对。".into())
                    },
                    Message::Operation,
                )
            }
            Message::CancelUpload(id) => {
                self.pending_operations += 1;
                let relay = self.host.relay.clone();
                blocking(
                    move || {
                        relay
                            .cancel_upload(OWNER_ID, &id)
                            .map_err(|e| e.to_string())?;
                        Ok("已取消未完成传输并释放预留空间。".into())
                    },
                    Message::Operation,
                )
            }
            Message::Operation(result) => {
                self.pending_operations = self.pending_operations.saturating_sub(1);
                match result {
                    Ok(message) => self.inform(message),
                    Err(error) => self.fail(error),
                }
                if self.ready_to_exit() {
                    self.stop_server()
                } else {
                    self.refresh()
                }
            }
            Message::Copy(value) => {
                self.inform("已复制。");
                clipboard::write(value)
            }
            Message::Details(id) => {
                self.selected_item = id;
                Task::none()
            }
            Message::Build(choice) => {
                self.composer = Composer::Text;
                if !self.sending {
                    self.feedback_build = choice;
                    self.attachments.clear();
                    self.send_request_id = None;
                    self.page = Page::Feed;
                }
                Task::none()
            }
            Message::Attachment(id, selected) => {
                if !self.sending {
                    if selected {
                        self.attachments.insert(id);
                    } else {
                        self.attachments.remove(&id);
                    }
                    self.send_request_id = None;
                }
                Task::none()
            }
            Message::Mark(id, status) => {
                self.pending_operations += 1;
                let relay = self.host.relay.clone();
                blocking(
                    move || {
                        relay
                            .patch_item(
                                OWNER_ID,
                                &id,
                                PatchItem {
                                    test_status: Some(status),
                                    ..PatchItem::default()
                                },
                            )
                            .map_err(|e| e.to_string())?;
                        Ok("已记录你确认的测试状态。".into())
                    },
                    Message::Operation,
                )
            }
            Message::SaveFile(id) => {
                self.pending_operations += 1;
                self.save_file(id)
            }
            Message::Export(id) => {
                self.pending_operations += 1;
                self.export(id)
            }
            Message::OpenInbox => {
                self.pending_operations += 1;
                let directory = self.host.directory.clone();
                blocking(
                    move || {
                        open_directory(&directory)?;
                        Ok("已打开数据目录。不要手工修改正在传输的临时文件。".into())
                    },
                    Message::Operation,
                )
            }
            Message::More => self.history(false),
            Message::Reload => Task::batch([self.history(true), self.refresh()]),
            Message::Stopped => iced::exit(),
        }
    }

    fn refresh(&mut self) -> Task<Message> {
        if self.snapshot_loading || self.exiting {
            self.refresh_pending = true;
            return Task::none();
        }
        self.snapshot_loading = true;
        let relay = self.host.relay.clone();
        blocking(
            move || relay.snapshot(OWNER_ID).map_err(|e| e.to_string()),
            Message::Snapshot,
        )
    }

    fn history(&mut self, reset: bool) -> Task<Message> {
        if self.history_loading || (!reset && self.next_cursor.is_none()) {
            return Task::none();
        }
        self.history_loading = true;
        let cursor = if reset {
            None
        } else {
            self.next_cursor.clone()
        };
        let relay = self.host.relay.clone();
        blocking(
            move || {
                relay
                    .list_items(OWNER_ID, cursor.as_deref(), 100)
                    .map_err(|e| e.to_string())
            },
            move |result| Message::History(result, reset),
        )
    }

    fn merge_items(&mut self, items: Vec<Item>) {
        for item in items {
            if let Some(existing) = self.items.iter_mut().find(|i| i.id == item.id) {
                *existing = item;
            } else {
                self.items.push(item);
            }
        }
        self.items.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
    }

    fn send(&mut self) -> Task<Message> {
        if self.sending || self.exiting {
            return Task::none();
        }
        let value = self.editor.text();
        if value.trim().is_empty() {
            self.fail("请先写一段文字或反馈说明。");
            return Task::none();
        }
        if value.len() > 64 * 1024 {
            self.fail("文字超过 64 KiB，请作为文本文件发送。");
            return Task::none();
        }
        self.sending = true;
        self.save_preferences();
        let request_id = self
            .send_request_id
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let project = self.project.trim().to_owned();
        let relay = self.host.relay.clone();
        if let Some(build) = &self.feedback_build {
            let feedback = NewFeedback {
                build_id: build.id.clone(),
                text: value,
                attachment_ids: self.attachments.iter().cloned().collect(),
                project,
                request_id,
            };
            blocking(
                move || {
                    relay
                        .create_feedback(OWNER_ID, feedback)
                        .map_err(|e| e.to_string())
                },
                Message::Sent,
            )
        } else {
            blocking(
                move || {
                    relay
                        .create_text(
                            OWNER_ID,
                            NewText {
                                text: value,
                                project,
                                request_id,
                            },
                        )
                        .map_err(|e| e.to_string())
                },
                Message::Sent,
            )
        }
    }

    fn enqueue(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        if self.exiting {
            return Task::none();
        }
        if self.queue.len() + paths.len() > 64 {
            self.fail("本机队列最多 64 个文件，请等待部分任务完成。");
            return Task::none();
        }
        let mut added = 0;
        for path in paths {
            if path.is_file() && !self.queue.iter().any(|queued| queued.path == path) {
                self.queue.push_back(QueuedFile {
                    path,
                    project: self.project.clone(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                });
                added += 1;
            } else if path.is_dir() {
                self.fail("请拖入文件；目录请先打包后发送。");
            }
        }
        if added > 0 {
            self.queue_paused = false;
            self.inform(format!("已添加 {added} 个文件，正在导入并校验。"));
            if !self.save_preferences() {
                self.queue_paused = true;
                return Task::none();
            }
        }
        self.next_import()
    }

    fn next_import(&mut self) -> Task<Message> {
        if self.importing.is_some() || self.queue_paused || self.exiting {
            return Task::none();
        }
        let Some(queued) = self.queue.front().cloned() else {
            return Task::none();
        };
        let path = queued.path;
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.importing = Some(ImportProgress {
            name: name.clone(),
            done: 0,
            total: 0,
        });
        let relay = self.host.relay.clone();
        let sender = self.host.sender.clone();
        let project = queued.project;
        let request_id = queued.request_id;
        blocking(
            move || {
                let mut last = Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .unwrap_or_else(Instant::now);
                relay
                    .import_file_with_request_id(&path, &project, &request_id, |done, total| {
                        if done == total || last.elapsed() >= Duration::from_millis(150) {
                            let _ = sender.send(Signal::ImportProgress {
                                name: name.clone(),
                                done,
                                total,
                            });
                            last = Instant::now();
                        }
                    })
                    .map_err(|e| e.to_string())
            },
            Message::Imported,
        )
    }

    fn rebuild_qr(&mut self) {
        self.qr = self
            .pairing_url()
            .and_then(|link| qr_code::Data::new(link).ok());
    }

    fn pairing_url(&self) -> Option<String> {
        let invite = self.invite.as_ref()?;
        let address = self.selected_address.as_ref()?;
        let mut url = url::Url::parse("lanrelay://pair").ok()?;
        url.query_pairs_mut()
            .append_pair("v", "2")
            .append_pair("url", address)
            .append_pair("token", &invite.token);
        Some(url.into())
    }

    fn save_file(&self, id: String) -> Task<Message> {
        let relay = self.host.relay.clone();
        Task::perform(
            async move {
                let info = tokio::task::spawn_blocking(move || {
                    relay.file_info(OWNER_ID, &id).map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())??;
                let Some(target) = rfd::AsyncFileDialog::new()
                    .set_title("保存原文件")
                    .set_file_name(&info.item.name)
                    .save_file()
                    .await
                else {
                    return Ok("已取消保存。".into());
                };
                let target = target.path().to_owned();
                tokio::task::spawn_blocking(move || {
                    copy_new(&info.path, &target, info.item.size)?;
                    Ok(format!("已保存：{}", target.display()))
                })
                .await
                .map_err(|e| e.to_string())?
            },
            Message::Operation,
        )
    }

    fn export(&self, id: String) -> Task<Message> {
        let relay = self.host.relay.clone();
        Task::perform(
            async move {
                let short: String = id.chars().take(8).collect();
                let content = tokio::task::spawn_blocking(move || {
                    relay
                        .export_feedback(OWNER_ID, &id)
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())??;
                let Some(target) = rfd::AsyncFileDialog::new()
                    .set_title("导出反馈给 Codex")
                    .set_file_name(format!("反馈-{short}.md"))
                    .save_file()
                    .await
                else {
                    return Ok("已取消导出。".into());
                };
                let target = target.path().to_owned();
                tokio::task::spawn_blocking(move || {
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&target)
                        .map_err(|e| format!("无法创建文件，请选择新的文件名：{e}"))?;
                    file.write_all(content.as_bytes())
                        .and_then(|_| file.sync_all())
                        .map_err(|e| e.to_string())?;
                    Ok(format!("反馈已导出：{}", target.display()))
                })
                .await
                .map_err(|e| e.to_string())?
            },
            Message::Operation,
        )
    }

    fn show_window(&mut self) -> Task<Message> {
        if self.exiting {
            return Task::none();
        }
        if let Some(id) = self.window {
            return Task::batch([
                window::set_mode(id, window::Mode::Windowed),
                window::gain_focus(id),
            ]);
        }
        let (id, open) = window::open(window_settings());
        self.window = Some(id);
        self.refresh_pending = false;
        Task::batch([open.map(|_| Message::Opened), self.refresh()])
    }

    fn fit_window(&self, initial: bool) -> Task<Message> {
        let Some(id) = self.window else {
            return Task::none();
        };
        work_area(id).map(move |area| Message::WorkArea(area, initial))
    }

    fn quit(&mut self) -> Task<Message> {
        if self.exiting {
            return Task::none();
        }
        let waiting = self.importing.is_some() || self.sending || self.pending_operations > 0;
        let wake = if waiting && self.window.is_none() {
            self.show_window()
        } else {
            Task::none()
        };
        self.exiting = true;
        self.queue_paused = true;
        self.save_preferences();
        if waiting {
            self.inform("正在等待当前文件或保存操作结束，再保存任务并退出。后续文件会保留在队列；若有保存对话框，请先完成或取消。");
            return wake;
        }
        self.inform("正在停止服务并保存已确认的传输位置…");
        self.stop_server()
    }

    fn ready_to_exit(&self) -> bool {
        self.exiting && self.importing.is_none() && !self.sending && self.pending_operations == 0
    }

    fn stop_server(&self) -> Task<Message> {
        let host = self.host.clone();
        Task::perform(
            async move {
                let server = host.server.lock().ok().and_then(|mut guard| guard.take());
                if let Some(server) = server {
                    server.shutdown().await;
                }
            },
            |_| Message::Stopped,
        )
    }

    fn save_preferences(&mut self) -> bool {
        let value = Preferences {
            project: self.project.clone(),
            address: self.selected_address.clone().unwrap_or_default(),
            pending_files: self.queue.iter().cloned().collect(),
        };
        // Small advisory preferences only. Transfer durability belongs to the
        // core. Failure does not advance or falsely complete any transfer.
        if let Err(error) = value.save(&self.host.directory) {
            self.fail(format!(
                "无法保存本机队列，请检查磁盘空间和目录权限：{error}"
            ));
            return false;
        }
        true
    }

    fn inform(&mut self, message: impl Into<String>) {
        self.notice = message.into();
        self.error = false;
    }
    fn fail(&mut self, error: impl Into<String>) {
        self.notice = error.into();
        self.error = true;
    }

    pub fn view(&self, _window: window::Id) -> Element<'_, Message> {
        let (devices, pending) = self.device_counts();
        let compact = self.viewport.width < 960.0;
        let (title, subtitle) = match self.page {
            Page::Feed => ("随手收发", "文件、文字与灵感，轻松到达另一端。"),
            Page::Transfers => ("传输动态", "每一次抵达，都清楚可见。"),
            Page::Devices => ("我的设备", "靠近一点，把手机和电脑连起来。"),
        };
        let connection = button(
            row![
                text("●").size(10).color(SAGE),
                text(if pending > 0 {
                    format!("{pending} 个连接待批准")
                } else {
                    format!("已配对 {devices} 台")
                })
                .size(13),
            ]
            .spacing(8)
            .align_y(alignment::Vertical::Center),
        )
        .padding([10, 14])
        .style(soft_button)
        .on_press(Message::Page(Page::Devices));
        let title_block = column![
            text(title)
                .size(if compact { 26 } else { 28 })
                .font(bold_font())
                .color(INK),
            text(subtitle).size(14).color(MUTED),
        ]
        .spacing(7);
        let heading: Element<'_, Message> = if compact {
            let mut compact_heading = column![
                row![
                    brand(false),
                    Space::new().width(Length::Fill),
                    connection,
                    button(text("退出").size(13))
                        .padding([9, 10])
                        .style(quiet_button)
                        .on_press(Message::Quit)
                ]
                .align_y(alignment::Vertical::Center)
                .spacing(8),
                row![
                    nav("↗", "收发", Page::Feed, self.page),
                    nav("⇄", "传输", Page::Transfers, self.page),
                    nav("◉", "设备", Page::Devices, self.page)
                ]
                .spacing(8),
            ]
            .spacing(if self.viewport.height < 600.0 { 14 } else { 20 });
            if self.viewport.height >= 600.0 {
                compact_heading = compact_heading.push(title_block);
            }
            compact_heading.into()
        } else {
            row![title_block, Space::new().width(Length::Fill), connection]
                .spacing(20)
                .align_y(alignment::Vertical::Center)
                .into()
        };
        let page = match self.page {
            Page::Feed => self.feed(),
            Page::Devices => self.devices(),
            Page::Transfers => self.transfers(),
        };
        let mut main = column![heading].spacing(20);
        if !self.notice.is_empty() && !self.notice.starts_with("此平台未启用托盘") {
            main = main.push(
                container(
                    row![
                        text(if self.error { "!" } else { "✓" })
                            .size(16)
                            .font(bold_font()),
                        text(&self.notice).size(13).width(Length::Fill)
                    ]
                    .spacing(10)
                    .align_y(alignment::Vertical::Center),
                )
                .padding([11, 14])
                .width(Length::Fill)
                .style(if self.error {
                    error_surface
                } else {
                    notice_surface
                }),
            );
        }
        main = main.push(page);
        let main = container(main)
            .padding(if compact { 20 } else { 28 })
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| tinted_surface(BACKGROUND, 0.0));
        if compact {
            main.into()
        } else {
            let pending_tasks = self.snapshot.as_ref().map_or(0, |s| {
                s.uploads
                    .iter()
                    .filter(|u| u.status == "uploading" || u.status == "failed")
                    .count()
            });
            let mut sidebar = column![
                brand(true),
                Space::new().height(30),
                nav("↗", "收发内容", Page::Feed, self.page),
                nav("⇄", "传输动态", Page::Transfers, self.page),
                nav("◉", "我的设备", Page::Devices, self.page),
            ]
            .spacing(10);
            if pending_tasks + self.queue.len() > 0 {
                sidebar = sidebar.push(
                    container(
                        text(format!("{} 项传输待完成", pending_tasks + self.queue.len()))
                            .size(12)
                            .color(MUTED),
                    )
                    .padding([5, 16]),
                );
            }
            sidebar = sidebar.push(Space::new().height(Length::Fill));
            sidebar = sidebar.push(
                container(
                    column![
                        text("留在你的设备之间")
                            .size(14)
                            .font(bold_font())
                            .color(SAGE),
                        text("不用绕路，随手分享。").size(12).color(MUTED),
                    ]
                    .spacing(7),
                )
                .padding(16)
                .width(Length::Fill)
                .style(sage_surface),
            );
            sidebar = sidebar.push(
                button(text("打开收件目录  ↗").size(13))
                    .on_press(Message::OpenInbox)
                    .padding([11, 12])
                    .style(quiet_button)
                    .width(Length::Fill),
            );
            sidebar = sidebar.push(
                row![
                    text(if self.tray_enabled {
                        "关窗收起到托盘"
                    } else {
                        "关窗退出服务"
                    })
                    .size(11)
                    .color(MUTED)
                    .width(Length::Fill),
                    button(text("退出").size(12))
                        .padding([7, 9])
                        .style(quiet_button)
                        .on_press(Message::Quit),
                ]
                .spacing(5)
                .align_y(alignment::Vertical::Center),
            );
            row![
                container(sidebar)
                    .padding([28, 20])
                    .width(218)
                    .height(Length::Fill)
                    .style(sidebar_surface),
                main
            ]
            .height(Length::Fill)
            .into()
        }
    }

    fn device_counts(&self) -> (usize, usize) {
        self.snapshot.as_ref().map_or((0, 0), |s| {
            (
                s.devices.iter().filter(|d| !d.revoked).count(),
                s.pair_requests
                    .iter()
                    .filter(|p| p.status == "pending" && p.expires_at > unix_millis())
                    .count(),
            )
        })
    }

    fn feed(&self) -> Element<'_, Message> {
        let mut compose = column![row![
            container(
                row![
                    segment("发送文件", Composer::Files, self.composer),
                    segment("写段文字", Composer::Text, self.composer),
                ]
                .spacing(4)
            )
            .padding(4)
            .style(segment_surface),
            Space::new().width(Length::Fill),
            button(
                text(if self.project.is_empty() {
                    "＋ 项目".into()
                } else {
                    format!("{}  ···", excerpt(&self.project, 14))
                })
                .size(13)
            )
            .padding([9, 12])
            .style(quiet_button)
            .on_press(Message::ProjectOptions),
        ]
        .spacing(8)
        .align_y(alignment::Vertical::Center),]
        .spacing(18);
        if self.project_options {
            compose = compose.push(
                row![
                    text("归入项目").size(13).color(MUTED),
                    text_input("可选，例如：应用试用、旅行照片", &self.project)
                        .on_input(Message::Project)
                        .padding([11, 13])
                        .size(14)
                        .style(field_style)
                        .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(alignment::Vertical::Center),
            );
        }
        if self.composer == Composer::Files {
            let picture = container(text("↗").size(42).font(bold_font()).color(SAGE))
                .center_x(80)
                .center_y(80)
                .style(icon_sage_surface);
            let introduction = column![
                text("轻轻一拖，随手分享")
                    .size(24)
                    .font(bold_font())
                    .color(INK),
                text("把原图、APK、录屏或文件放进来。")
                    .size(14)
                    .color(MUTED),
                row![
                    badge("原始画质", SAGE, SAGE_SOFT),
                    badge("断点续传", MAUVE, LAVENDER)
                ]
                .spacing(8),
            ]
            .spacing(10);
            let choose = button(text("选择文件  ↗").size(15).font(bold_font()))
                .on_press(Message::ChooseFiles)
                .padding([13, 20])
                .style(primary_button);
            let drop_content: Element<'_, Message> = if self.viewport.width >= 1080.0 {
                row![picture, introduction.width(Length::Fill), choose]
                    .spacing(20)
                    .align_y(alignment::Vertical::Center)
                    .into()
            } else {
                column![
                    row![picture, introduction.width(Length::Fill)]
                        .spacing(18)
                        .align_y(alignment::Vertical::Center),
                    choose
                ]
                .spacing(18)
                .into()
            };
            compose = compose.push(
                container(drop_content)
                    .padding(24)
                    .width(Length::Fill)
                    .style(drop_surface),
            );
        } else {
            if let Some(build) = &self.feedback_build {
                compose = compose.push(
                    container(
                        row![
                            column![
                                text("构建反馈").size(12).color(MAUVE),
                                text(&build.label).size(15).font(bold_font())
                            ]
                            .spacing(5)
                            .width(Length::Fill),
                            button(text("取消关联").size(13))
                                .on_press(Message::Build(None))
                                .style(quiet_button)
                                .padding([9, 12]),
                        ]
                        .spacing(12)
                        .align_y(alignment::Vertical::Center),
                    )
                    .padding(14)
                    .width(Length::Fill)
                    .style(lavender_surface),
                );
            }
            compose = compose.push(
                text_editor(&self.editor)
                    .placeholder(if self.feedback_build.is_some() {
                        "写下遇到的问题和复现步骤，再从下方内容里勾选截图、录屏。"
                    } else {
                        "有什么想分享的？文字、链接，或一小段代码。"
                    })
                    .height(126)
                    .padding(16)
                    .size(16)
                    .style(editor_style)
                    .on_action(Message::Editor),
            );
            let label = if self.sending {
                "正在发送…"
            } else if self.feedback_build.is_some() {
                "发送反馈  ↗"
            } else {
                "发送文字  ↗"
            };
            let mut send = button(text(label).size(14).font(bold_font()))
                .padding([12, 18])
                .style(primary_button);
            if !self.sending && !self.exiting {
                send = send.on_press(Message::Send);
            }
            compose = compose.push(
                row![
                    text(if self.feedback_build.is_some() {
                        format!("已选 {} 个附件", self.attachments.len())
                    } else {
                        "保留换行、缩进与每一个字。".into()
                    })
                    .size(13)
                    .color(MUTED)
                    .width(Length::Fill),
                    send,
                ]
                .spacing(12)
                .align_y(alignment::Vertical::Center),
            );
        }
        let query = self.query.to_lowercase();
        let visible = self.items.iter().filter(|item| {
            let matches_project = match &self.filter {
                ProjectFilter::All => true,
                ProjectFilter::Unassigned => item.project.is_empty(),
                ProjectFilter::Named(project) => item.project == *project,
            };
            matches_project
                && (query.is_empty()
                    || format!("{} {} {} {}", item.name, item.text, item.project, item.note)
                        .to_lowercase()
                        .contains(&query))
        });
        let mut projects: Vec<String> = self
            .items
            .iter()
            .filter(|i| !i.project.is_empty())
            .map(|i| i.project.clone())
            .collect();
        projects.sort();
        projects.dedup();
        let filters: Vec<_> = [ProjectFilter::All, ProjectFilter::Unassigned]
            .into_iter()
            .chain(projects.into_iter().map(ProjectFilter::Named))
            .collect();
        let heading = row![
            text("最近内容").size(20).font(bold_font()),
            badge(format!("{}", self.items.len()), MUTED, SAGE_SOFT),
            Space::new().width(Length::Fill),
            button(
                text(if self.history_loading {
                    "加载中…"
                } else {
                    "刷新  ↻"
                })
                .size(13)
            )
            .padding([9, 12])
            .style(quiet_button)
            .on_press(Message::Reload),
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Center);
        let search = row![
            text_input("搜索已加载的文件、文字、项目", &self.query)
                .on_input(Message::Query)
                .padding([12, 14])
                .size(14)
                .style(field_style)
                .width(Length::Fill),
            pick_list(filters, Some(self.filter.clone()), Message::Filter)
                .width(155)
                .padding([12, 13])
                .text_size(13)
                .style(select_style),
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Center);
        let mut content = column![
            container(compose)
                .padding(20)
                .width(Length::Fill)
                .style(card),
            Space::new().height(5),
            heading,
            search
        ]
        .spacing(14);
        let mut count = 0;
        for item in visible {
            content = content.push(self.item_card(item));
            count += 1;
        }
        if count == 0 {
            let (devices, _) = self.device_counts();
            let mut empty = column![
                container(
                    text(if self.query.is_empty() { "↗" } else { "⌕" })
                        .size(30)
                        .color(SAGE)
                )
                .center_x(60)
                .center_y(60)
                .style(icon_sage_surface),
                text(if self.query.is_empty() {
                    "从一份小小的分享开始"
                } else {
                    "还没有找到匹配的内容"
                })
                .size(18)
                .font(bold_font()),
                text(if self.query.is_empty() {
                    "电脑拖入文件，手机也可以通过系统分享发过来。"
                } else {
                    "试试更短的关键词，或切换项目。搜索范围为已加载内容。"
                })
                .size(14)
                .color(MUTED),
            ]
            .spacing(12)
            .align_x(alignment::Horizontal::Center);
            if devices == 0 && self.query.is_empty() {
                empty = empty.push(
                    button(text("连接第一台设备  →").size(14))
                        .on_press(Message::Page(Page::Devices))
                        .padding([11, 17])
                        .style(soft_button),
                );
            }
            content = content.push(
                container(empty)
                    .padding([28, 20])
                    .center_x(Length::Fill)
                    .style(card),
            );
        }
        if self.next_cursor.is_some() {
            let mut more = button(
                text(if self.history_loading {
                    "正在加载…"
                } else {
                    "加载更早的内容  ↓"
                })
                .size(14),
            )
            .padding([13, 20])
            .style(soft_button);
            if !self.history_loading {
                more = more.on_press(Message::More);
            }
            content = content.push(container(more).center_x(Length::Fill).padding([8, 0]));
        }
        scrollable(content.padding([0, 3]))
            .height(Length::Fill)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new()
                    .width(4)
                    .margin(3)
                    .scroller_width(4),
            ))
            .style(scroll_style)
            .into()
    }

    fn item_card<'a>(&'a self, item: &'a Item) -> Element<'a, Message> {
        let file = item.kind == "file";
        let apk = file && item.name.to_lowercase().ends_with(".apk");
        let (label, tint, soft) = if apk {
            ("APK", MAUVE, LAVENDER)
        } else if item.mime.starts_with("image/") {
            ("IMG", CLAY, APRICOT)
        } else if item.mime.starts_with("video/") {
            ("VID", BLUE, BLUE_SOFT)
        } else if file {
            ("FILE", SAGE, SAGE_SOFT)
        } else if item.kind == "feedback" {
            ("↩", MAUVE, LAVENDER)
        } else {
            ("TXT", CLAY, APRICOT)
        };
        let symbol = container(
            text(label)
                .size(if label.len() > 3 { 11 } else { 14 })
                .font(bold_font())
                .color(tint),
        )
        .center_x(50)
        .center_y(54)
        .style(move |_| tinted_surface(soft, 15.0));
        let sender = if item.device_id == OWNER_ID {
            "这台电脑".to_owned()
        } else {
            self.snapshot
                .as_ref()
                .and_then(|s| s.devices.iter().find(|d| d.id == item.device_id))
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "已配对设备".into())
        };
        let title = if file {
            item.name.clone()
        } else if item.kind == "feedback" {
            "一份测试反馈".into()
        } else {
            "一段文字".into()
        };
        let mut meta = row![text(if file {
            format!("{}  ·  来自 {sender}", bytes(item.size))
        } else {
            format!("来自 {sender}")
        })
        .size(12)
        .color(MUTED)]
        .spacing(8);
        if !item.project.is_empty() {
            meta = meta.push(badge(excerpt(&item.project, 20), SAGE, SAGE_SOFT));
        }
        if apk {
            meta = meta.push(badge(
                test_status(&item.test_status),
                if item.test_status == "failed" {
                    RED
                } else {
                    MAUVE
                },
                if item.test_status == "failed" {
                    RED_SOFT
                } else {
                    LAVENDER
                },
            ));
        }
        let mut detail = column![
            text(title)
                .size(16)
                .font(bold_font())
                .wrapping(text::Wrapping::Glyph),
            meta.wrap()
        ]
        .spacing(9)
        .width(Length::Fill);
        if !file {
            detail = detail.push(text(excerpt(&item.text, 200)).size(15).line_height(1.55));
        }
        if let Some(metadata) = item.apk.as_ref().filter(|_| apk) {
            if let Some(version) = &metadata.version_name {
                detail = detail.push(text(format!("版本 {version}")).size(12).color(MUTED));
            }
        }
        let mut body = column![row![symbol, detail]
            .spacing(16)
            .align_y(alignment::Vertical::Top)]
        .spacing(16);
        let mut actions = row![].spacing(7).align_y(alignment::Vertical::Center);
        if file {
            actions = actions.push(
                button(text("保存原件").size(13))
                    .on_press(Message::SaveFile(item.id.clone()))
                    .padding([9, 13])
                    .style(soft_button),
            );
            if apk {
                actions = actions.push(
                    button(text("写反馈").size(13))
                        .on_press(Message::Build(Some(BuildChoice {
                            id: item.id.clone(),
                            label: item.name.clone(),
                        })))
                        .padding([9, 13])
                        .style(quiet_button),
                );
            }
            if self
                .feedback_build
                .as_ref()
                .is_some_and(|b| b.id != item.id)
            {
                let id = item.id.clone();
                actions = actions.push(
                    checkbox(self.attachments.contains(&item.id))
                        .label("作为反馈附件")
                        .text_size(13)
                        .on_toggle(move |value| Message::Attachment(id.clone(), value)),
                );
            }
        } else {
            actions = actions.push(
                button(text("复制全文").size(13))
                    .on_press(Message::Copy(item.text.clone()))
                    .padding([9, 13])
                    .style(soft_button),
            );
            if item.kind == "feedback" {
                actions = actions.push(
                    button(text("导出给 Codex  ↗").size(13))
                        .on_press(Message::Export(item.id.clone()))
                        .padding([9, 13])
                        .style(quiet_button),
                );
            }
        }
        let expanded = self.selected_item.as_deref() == Some(item.id.as_str());
        actions = actions.push(
            button(
                text(if expanded {
                    "收起详情  ↑"
                } else {
                    "更多详情  ↓"
                })
                .size(13),
            )
            .on_press(Message::Details(if expanded {
                None
            } else {
                Some(item.id.clone())
            }))
            .padding([9, 13])
            .style(quiet_button),
        );
        body = body.push(actions.wrap());
        if expanded {
            let mut extra = column![].spacing(12);
            if !item.text.is_empty() {
                extra = extra.push(text(&item.text).size(15).line_height(1.6));
            }
            if let Some(metadata) = &item.apk {
                extra = extra.push(
                    text(format!(
                        "包名  {}\n版本  {}  ·  版本号 {}",
                        metadata.package_name.as_deref().unwrap_or("未知"),
                        metadata.version_name.as_deref().unwrap_or("未知"),
                        metadata.version_code.as_deref().unwrap_or("未知")
                    ))
                    .size(13)
                    .color(MUTED)
                    .wrapping(text::Wrapping::Glyph),
                );
            }
            if !item.sha256.is_empty() {
                extra = extra.push(
                    text(format!("SHA-256\n{}", item.sha256))
                        .size(12)
                        .color(MUTED)
                        .wrapping(text::Wrapping::Glyph),
                );
            }
            extra = extra.push(
                text(format!("记录编号  {}", item.id))
                    .size(12)
                    .color(MUTED)
                    .wrapping(text::Wrapping::Glyph),
            );
            if apk {
                extra = extra.push(text("手动记录你的试用结果").size(14).font(bold_font()));
                let mut marks = row![].spacing(7);
                for (value, label) in [
                    ("unverified", "待验收"),
                    ("installed", "已安装"),
                    ("passed", "通过"),
                    ("failed", "失败"),
                    ("blocked", "受阻"),
                ] {
                    marks = marks.push(
                        button(text(label).size(13))
                            .on_press(Message::Mark(item.id.clone(), value.into()))
                            .padding([9, 12])
                            .style(if item.test_status == value {
                                primary_button
                            } else {
                                soft_button
                            }),
                    );
                }
                extra = extra.push(marks.wrap()).push(
                    text("下载完成不会自动变更安装或测试状态。")
                        .size(12)
                        .color(MUTED),
                );
            }
            body = body.push(
                container(extra)
                    .padding(16)
                    .width(Length::Fill)
                    .style(inset_surface),
            );
        }
        container(body)
            .padding(20)
            .width(Length::Fill)
            .style(card)
            .into()
    }

    fn devices(&self) -> Element<'_, Message> {
        let mut pairing = column![
            row![
                text("连接新设备").size(20).font(bold_font()),
                Space::new().width(Length::Fill),
                badge("在同一局域网", SAGE, SAGE_SOFT)
            ]
            .align_y(alignment::Vertical::Center),
            text("打开手机上的随手传，扫描这里的二维码，再由你批准连接。")
                .size(14)
                .color(MUTED),
        ]
        .spacing(14);
        if let Some(invite) = &self.invite {
            let remaining = (invite.expires_at - unix_millis()).max(0) / 1000;
            if remaining > 0 {
                let display_code = invite
                    .code
                    .chars()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                let mut code = column![
                    text("也可以输入连接码").size(13).color(MUTED),
                    text(display_code).size(32).font(bold_font()).color(SAGE),
                    text(format!("{} 分 {} 秒内有效", remaining / 60, remaining % 60))
                        .size(12)
                        .color(MUTED),
                ]
                .spacing(12)
                .width(Length::Fill);
                if let Some(link) = self.pairing_url() {
                    code = code.push(
                        button(text("复制邀请链接  ↗").size(13))
                            .on_press(Message::Copy(link))
                            .padding([10, 14])
                            .style(soft_button),
                    );
                }
                if let Some(qr) = &self.qr {
                    let qr_view = container(qr_code(qr).cell_size(3)).padding(16).style(card);
                    let block: Element<'_, Message> = if self.viewport.width < 720.0 {
                        column![qr_view, code]
                            .spacing(22)
                            .align_x(alignment::Horizontal::Center)
                            .into()
                    } else {
                        row![qr_view, code]
                            .spacing(28)
                            .align_y(alignment::Vertical::Center)
                            .into()
                    };
                    pairing = pairing.push(
                        container(block)
                            .padding(20)
                            .width(Length::Fill)
                            .style(sage_surface),
                    );
                } else {
                    pairing = pairing.push(code);
                }
            } else {
                pairing = pairing.push(
                    container(text("这组连接码已过期，换一组就好。").size(14).color(MUTED))
                        .padding(20)
                        .width(Length::Fill)
                        .style(inset_surface),
                );
            }
        } else {
            pairing = pairing.push(
                container(
                    column![
                        text("◉").size(34).color(SAGE),
                        text(if self.invite_loading {
                            "正在准备连接邀请…"
                        } else {
                            "生成邀请，让设备相遇"
                        })
                        .size(16)
                        .font(bold_font()),
                    ]
                    .spacing(14)
                    .align_x(alignment::Horizontal::Center),
                )
                .padding(28)
                .center_x(Length::Fill)
                .style(sage_surface),
            );
        }
        if self.addresses.is_empty() {
            pairing = pairing.push(
                text("连接 Wi-Fi 或网线后，重新生成邀请以发现局域网地址。")
                    .size(14)
                    .color(RED),
            );
        } else {
            pairing = pairing.push(text("使用的网络地址").size(12).color(MUTED));
            pairing = pairing.push(
                pick_list(
                    self.addresses.clone(),
                    self.selected_address.clone(),
                    Message::Address,
                )
                .width(Length::Fill)
                .padding([12, 14])
                .text_size(14)
                .style(select_style),
            );
        }
        let mut renew = button(
            text(if self.invite_loading {
                "准备中…"
            } else {
                "换一组连接码  ↻"
            })
            .size(14),
        )
        .padding([12, 18])
        .style(primary_button);
        if !self.invite_loading {
            renew = renew.on_press(Message::NewInvite);
        }
        pairing = pairing.push(renew);
        pairing = pairing.push(
            text("HTTP 局域网模式未加密，请只在可信网络中使用。")
                .size(12)
                .color(MUTED),
        );
        let mut content = column![container(pairing)
            .padding(24)
            .width(Length::Fill)
            .style(card)]
        .spacing(20);
        if let Some(snapshot) = &self.snapshot {
            let pending: Vec<_> = snapshot
                .pair_requests
                .iter()
                .filter(|p| p.status == "pending" && p.expires_at > unix_millis())
                .collect();
            if !pending.is_empty() {
                content = content.push(text("有人想与你连接").size(19).font(bold_font()));
                for request in pending {
                    let actions = row![
                        button(text("暂不连接").size(13))
                            .on_press(Message::Pair(request.id.clone(), false))
                            .padding([10, 13])
                            .style(quiet_button),
                        button(text("批准连接").size(13))
                            .on_press(Message::Pair(request.id.clone(), true))
                            .padding([10, 16])
                            .style(primary_button),
                    ]
                    .spacing(8);
                    content = content.push(
                        container(
                            column![
                                row![
                                    device_symbol(),
                                    column![
                                        text(&request.name).size(17).font(bold_font()),
                                        text("新的连接申请").size(12).color(CLAY)
                                    ]
                                    .spacing(7)
                                    .width(Length::Fill)
                                ]
                                .spacing(15)
                                .align_y(alignment::Vertical::Center),
                                actions,
                            ]
                            .spacing(16),
                        )
                        .padding(20)
                        .width(Length::Fill)
                        .style(apricot_surface),
                    );
                }
            }
            let approved: Vec<_> = snapshot.devices.iter().filter(|d| !d.revoked).collect();
            content = content.push(
                row![
                    text("已配对设备").size(19).font(bold_font()),
                    badge(format!("{}", approved.len()), MUTED, SAGE_SOFT)
                ]
                .spacing(10)
                .align_y(alignment::Vertical::Center),
            );
            if approved.is_empty() {
                content = content.push(
                    container(
                        column![
                            text("你的设备，等你连接").size(16).font(bold_font()),
                            text("配对一次，之后可以从手机的最近连接中找回这台电脑。")
                                .size(14)
                                .color(MUTED)
                        ]
                        .spacing(10),
                    )
                    .padding(24)
                    .width(Length::Fill)
                    .style(card),
                );
            }
            for device in approved {
                let details = column![
                    text(&device.name).size(17).font(bold_font()),
                    text("共享本电脑的收件区").size(12).color(MUTED)
                ]
                .spacing(7)
                .width(Length::Fill);
                let mut content_line =
                    column![
                        row![device_symbol(), details, badge("已配对", SAGE, SAGE_SOFT)]
                            .spacing(15)
                            .align_y(alignment::Vertical::Center)
                    ]
                    .spacing(16);
                if self.revoke_prompt.as_deref() == Some(&device.id) {
                    content_line = content_line.push(
                        text("撤销后，这台设备需要重新获得你的批准。")
                            .size(13)
                            .color(RED),
                    );
                    content_line = content_line.push(
                        row![
                            button(text("保留连接").size(13))
                                .on_press(Message::RevokePrompt(None))
                                .padding([10, 14])
                                .style(soft_button),
                            button(text("确认撤销").size(13))
                                .on_press(Message::Revoke(device.id.clone()))
                                .padding([10, 14])
                                .style(danger_button),
                        ]
                        .spacing(10),
                    );
                } else {
                    content_line = content_line.push(
                        button(text("撤销连接").size(13))
                            .on_press(Message::RevokePrompt(Some(device.id.clone())))
                            .padding([9, 12])
                            .style(quiet_button),
                    );
                }
                content = content.push(
                    container(content_line)
                        .padding(20)
                        .width(Length::Fill)
                        .style(card),
                );
            }
            content = content.push(
                text("已批准设备共享内容区，项目名称只用于整理。")
                    .size(12)
                    .color(MUTED),
            );
        }
        scrollable(content.padding([0, 3]))
            .height(Length::Fill)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new()
                    .width(4)
                    .margin(3)
                    .scroller_width(4),
            ))
            .style(scroll_style)
            .into()
    }

    fn transfers(&self) -> Element<'_, Message> {
        let mut content = column![].spacing(16);
        if let Some(progress) = &self.importing {
            let ratio = fraction(progress.done, progress.total);
            content = content.push(
                container(
                    column![
                        row![
                            badge("电脑正在准备", SAGE, SAGE_SOFT),
                            Space::new().width(Length::Fill),
                            text(format!("{:.0}%", ratio * 100.0))
                                .size(20)
                                .font(bold_font())
                                .color(SAGE)
                        ]
                        .align_y(alignment::Vertical::Center),
                        text(&progress.name)
                            .size(17)
                            .font(bold_font())
                            .wrapping(text::Wrapping::Glyph),
                        progress_bar(0.0..=1.0, ratio)
                            .girth(7)
                            .style(progress_style),
                        text(format!(
                            "已处理 {} / {}",
                            bytes(progress.done),
                            bytes(progress.total)
                        ))
                        .size(13)
                        .color(MUTED),
                        text("本机导入与校验，完成后手机即可下载。")
                            .size(12)
                            .color(MUTED),
                    ]
                    .spacing(14),
                )
                .padding(24)
                .width(Length::Fill)
                .style(card),
            );
        }
        if !self.queue.is_empty() {
            content = content.push(
                row![
                    text("本机发送队列")
                        .size(19)
                        .font(bold_font())
                        .width(Length::Fill),
                    button(
                        text(if self.queue_paused {
                            "继续队列  →"
                        } else {
                            "暂停后续文件"
                        })
                        .size(13)
                    )
                    .on_press(Message::ToggleQueue)
                    .padding([10, 14])
                    .style(soft_button),
                ]
                .spacing(10)
                .align_y(alignment::Vertical::Center),
            );
            for (index, queued) in self.queue.iter().enumerate() {
                let name = queued
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let mut line = row![
                    container(text("FILE").size(10).font(bold_font()).color(SAGE))
                        .center_x(44)
                        .center_y(46)
                        .style(icon_sage_surface),
                    column![
                        text(name).size(15).wrapping(text::Wrapping::Glyph),
                        text(if queued.project.is_empty() {
                            "未分项目"
                        } else {
                            &queued.project
                        })
                        .size(12)
                        .color(MUTED)
                    ]
                    .spacing(7)
                    .width(Length::Fill)
                ]
                .spacing(14)
                .align_y(alignment::Vertical::Center);
                if !(index == 0 && self.importing.is_some()) {
                    line = line.push(
                        button(text("移出").size(13))
                            .on_press(Message::RemoveQueued(index))
                            .padding([9, 12])
                            .style(quiet_button),
                    );
                }
                content = content.push(container(line).padding(18).width(Length::Fill).style(card));
            }
        }
        let pending: Vec<_> = self
            .snapshot
            .as_ref()
            .map(|s| {
                s.uploads
                    .iter()
                    .filter(|u| {
                        u.status != "complete" && u.status != "cancelled" && u.device_id != OWNER_ID
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !pending.is_empty() {
            content = content.push(text("从设备接收").size(19).font(bold_font()));
            for upload in &pending {
                let ratio = fraction(upload.offset, upload.size);
                let rate = self
                    .transfer_samples
                    .get(&upload.id)
                    .and_then(TransferSample::rate);
                let state = if upload.status == "failed" {
                    "需要处理"
                } else if upload.offset >= upload.size {
                    "等待校验确认"
                } else if rate.is_some() {
                    "正在接收"
                } else if upload.offset == 0 {
                    "等待发送"
                } else {
                    "等待继续"
                };
                let mut details = column![
                    row![
                        badge(
                            state,
                            if upload.status == "failed" { RED } else { SAGE },
                            if upload.status == "failed" {
                                RED_SOFT
                            } else {
                                SAGE_SOFT
                            }
                        ),
                        Space::new().width(Length::Fill),
                        text(format!("{:.0}%", ratio * 100.0))
                            .size(20)
                            .font(bold_font())
                            .color(SAGE)
                    ]
                    .align_y(alignment::Vertical::Center),
                    text(&upload.name)
                        .size(17)
                        .font(bold_font())
                        .wrapping(text::Wrapping::Glyph),
                    progress_bar(0.0..=1.0, ratio)
                        .girth(7)
                        .style(progress_style),
                    text(format!(
                        "已确认 {} / {}",
                        bytes(upload.offset),
                        bytes(upload.size)
                    ))
                    .size(13)
                    .color(MUTED),
                ]
                .spacing(14);
                if let Some(speed) = rate {
                    details = details.push(
                        row![
                            text(format!("{}/s", bytes(speed as u64)))
                                .size(18)
                                .font(bold_font())
                                .color(SAGE),
                            text(remaining_time(
                                upload.size.saturating_sub(upload.offset),
                                speed
                            ))
                            .size(12)
                            .color(MUTED)
                        ]
                        .spacing(12)
                        .align_y(alignment::Vertical::Center),
                    );
                }
                details = details.push(
                    row![
                        text("在发送设备暂停或继续")
                            .size(12)
                            .color(MUTED)
                            .width(Length::Fill),
                        button(text("取消传输").size(13))
                            .on_press(Message::CancelUpload(upload.id.clone()))
                            .padding([9, 12])
                            .style(quiet_button)
                    ]
                    .spacing(10)
                    .align_y(alignment::Vertical::Center),
                );
                content = content.push(
                    container(details)
                        .padding(24)
                        .width(Length::Fill)
                        .style(card),
                );
            }
        }
        if self.queue.is_empty() && self.importing.is_none() && pending.is_empty() {
            content = content.push(
                container(
                    column![
                        container(text("⇄").size(38).color(SAGE))
                            .center_x(80)
                            .center_y(80)
                            .style(icon_sage_surface),
                        text("此刻，传输很轻松").size(23).font(bold_font()),
                        text("没有待完成的任务。下一份内容，随时出发。")
                            .size(14)
                            .color(MUTED),
                        button(text("去分享点什么  ↗").size(14))
                            .on_press(Message::Page(Page::Feed))
                            .padding([12, 18])
                            .style(primary_button),
                    ]
                    .spacing(16)
                    .align_x(alignment::Horizontal::Center),
                )
                .padding([32, 22])
                .center_x(Length::Fill)
                .style(card),
            );
        }
        if let Some(snapshot) = &self.snapshot {
            let finished: Vec<_> = snapshot
                .uploads
                .iter()
                .filter(|u| u.status == "complete")
                .take(6)
                .collect();
            if !finished.is_empty() {
                content = content.push(text("最近完成").size(19).font(bold_font()));
                for upload in finished {
                    content = content.push(
                        container(
                            row![
                                container(text("✓").size(19).color(SAGE))
                                    .center_x(40)
                                    .center_y(42)
                                    .style(icon_sage_surface),
                                column![
                                    text(&upload.name).size(15).wrapping(text::Wrapping::Glyph),
                                    text(format!("{}  ·  完整性校验通过", bytes(upload.size)))
                                        .size(12)
                                        .color(MUTED)
                                ]
                                .spacing(6)
                                .width(Length::Fill),
                            ]
                            .spacing(14)
                            .align_y(alignment::Vertical::Center),
                        )
                        .padding(17)
                        .width(Length::Fill)
                        .style(card),
                    );
                }
            }
            content = content.push(
                container(
                    column![
                        row![
                            text("电脑收件区")
                                .size(16)
                                .font(bold_font())
                                .width(Length::Fill),
                            button(text("打开目录  ↗").size(13))
                                .on_press(Message::OpenInbox)
                                .padding([9, 12])
                                .style(quiet_button)
                        ]
                        .align_y(alignment::Vertical::Center),
                        progress_bar(
                            0.0..=1.0,
                            fraction(snapshot.used_bytes, snapshot.max_storage_bytes)
                        )
                        .girth(5)
                        .style(progress_style),
                        text(format!(
                            "已使用 {} / {}",
                            bytes(snapshot.used_bytes),
                            bytes(snapshot.max_storage_bytes)
                        ))
                        .size(13)
                        .color(MUTED),
                    ]
                    .spacing(12),
                )
                .padding(20)
                .width(Length::Fill)
                .style(sage_surface),
            );
        }
        content = content.push(
            text(format!("随手传 {}  ·  协议 2", env!("CARGO_PKG_VERSION")))
                .size(12)
                .color(MUTED),
        );
        scrollable(content.padding([0, 3]))
            .height(Length::Fill)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new()
                    .width(4)
                    .margin(3)
                    .scroller_width(4),
            ))
            .style(scroll_style)
            .into()
    }
}

fn signal_stream() -> impl iced::futures::Stream<Item = Message> {
    let receiver = SIGNALS
        .get()
        .expect("signals initialized before UI")
        .lock()
        .expect("signals lock")
        .take()
        .expect("one event subscription");
    iced::futures::stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|signal| (Message::Signal(signal), receiver))
    })
}

fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
    map: impl FnOnce(Result<T, String>) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(job)
                .await
                .map_err(|e| format!("后台任务未完成：{e}"))
                .and_then(|result| result)
        },
        map,
    )
}

const BACKGROUND: Color = Color::from_rgb8(247, 248, 244);
const INK: Color = Color::from_rgb8(36, 52, 43);
const MUTED: Color = Color::from_rgb8(95, 109, 100);
const LINE: Color = Color::from_rgb8(228, 233, 226);
const SAGE: Color = Color::from_rgb8(83, 119, 98);
const SAGE_SOFT: Color = Color::from_rgb8(234, 241, 233);
const LAVENDER: Color = Color::from_rgb8(238, 234, 244);
const MAUVE: Color = Color::from_rgb8(119, 102, 143);
const APRICOT: Color = Color::from_rgb8(246, 229, 216);
const CLAY: Color = Color::from_rgb8(151, 107, 76);
const BLUE: Color = Color::from_rgb8(91, 125, 145);
const BLUE_SOFT: Color = Color::from_rgb8(232, 240, 244);
const RED: Color = Color::from_rgb8(164, 73, 62);
const RED_SOFT: Color = Color::from_rgb8(249, 235, 230);

pub fn theme() -> Theme {
    Theme::custom(
        "Sage Relay",
        iced::theme::Palette {
            background: BACKGROUND,
            text: INK,
            primary: SAGE,
            success: SAGE,
            danger: RED,
            warning: CLAY,
        },
    )
}

pub fn default_font() -> Font {
    #[cfg(windows)]
    {
        Font::with_name("Microsoft YaHei UI")
    }
    #[cfg(target_os = "linux")]
    {
        Font::with_name("Noto Sans CJK SC")
    }
    #[cfg(target_os = "macos")]
    {
        Font::with_name("PingFang SC")
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Font::DEFAULT
    }
}

fn bold_font() -> Font {
    Font {
        weight: iced::font::Weight::Semibold,
        ..default_font()
    }
}

fn brand(full: bool) -> Element<'static, Message> {
    let mark = container(text("↗").size(23).font(bold_font()).color(Color::WHITE))
        .center_x(42)
        .center_y(42)
        .style(|_| tinted_surface(SAGE, 14.0));
    let mut label = column![text("随手传").size(21).font(bold_font()).color(INK)].spacing(3);
    if full {
        label = label.push(text("LINK A LITTLE CLOSER").size(8).color(MUTED));
    }
    row![mark, label]
        .spacing(11)
        .align_y(alignment::Vertical::Center)
        .into()
}

fn badge(
    label: impl Into<String>,
    foreground: Color,
    background: Color,
) -> Element<'static, Message> {
    container(text(label.into()).size(11).color(foreground))
        .padding([5, 10])
        .style(move |_| tinted_surface(background, 99.0))
        .into()
}

fn device_symbol() -> Element<'static, Message> {
    container(text("◉").size(24).color(SAGE))
        .center_x(48)
        .center_y(52)
        .style(icon_sage_surface)
        .into()
}

fn tinted_surface(color: Color, radius: f32) -> container::Style {
    container::Style {
        background: Some(color.into()),
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

fn card(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 22.0.into(),
        },
        ..tinted_surface(Color::WHITE, 22.0)
    }
}

fn sidebar_surface(_theme: &Theme) -> container::Style {
    tinted_surface(Color::from_rgb8(241, 244, 237), 0.0)
}
fn sage_surface(_theme: &Theme) -> container::Style {
    tinted_surface(SAGE_SOFT, 18.0)
}
fn icon_sage_surface(_theme: &Theme) -> container::Style {
    tinted_surface(SAGE_SOFT, 18.0)
}
fn lavender_surface(_theme: &Theme) -> container::Style {
    tinted_surface(LAVENDER, 16.0)
}
fn apricot_surface(_theme: &Theme) -> container::Style {
    tinted_surface(APRICOT, 20.0)
}
fn inset_surface(_theme: &Theme) -> container::Style {
    tinted_surface(BACKGROUND, 16.0)
}
fn segment_surface(_theme: &Theme) -> container::Style {
    tinted_surface(BACKGROUND, 14.0)
}
fn drop_surface(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 18.0.into(),
        },
        ..tinted_surface(Color::from_rgb8(248, 250, 246), 18.0)
    }
}
fn error_surface(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(RED),
        ..tinted_surface(RED_SOFT, 12.0)
    }
}
fn notice_surface(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(SAGE),
        ..tinted_surface(SAGE_SOFT, 12.0)
    }
}

fn button_surface(background: Color, foreground: Color, status: button::Status) -> button::Style {
    let mut fill = background;
    let mut text_color = foreground;
    match status {
        button::Status::Hovered if background.a == 0.0 => {
            fill = SAGE_SOFT;
        }
        button::Status::Pressed if background.a == 0.0 => {
            fill = LINE;
        }
        button::Status::Hovered => {
            fill = Color::from_rgba(
                background.r * 0.96,
                background.g * 0.97,
                background.b * 0.96,
                background.a.max(0.08),
            );
        }
        button::Status::Pressed => {
            fill = Color::from_rgba(
                background.r * 0.92,
                background.g * 0.94,
                background.b * 0.92,
                background.a.max(0.12),
            );
        }
        button::Status::Disabled => {
            fill.a *= 0.4;
            text_color.a *= 0.55;
        }
        button::Status::Active => {}
    }
    button::Style {
        background: Some(fill.into()),
        text_color,
        border: Border {
            radius: 12.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}
fn primary_button(_theme: &Theme, status: button::Status) -> button::Style {
    button_surface(SAGE, Color::WHITE, status)
}
fn soft_button(_theme: &Theme, status: button::Status) -> button::Style {
    button_surface(SAGE_SOFT, SAGE, status)
}
fn quiet_button(_theme: &Theme, status: button::Status) -> button::Style {
    button_surface(Color::TRANSPARENT, MUTED, status)
}
fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    button_surface(RED, Color::WHITE, status)
}

fn nav<'a>(icon: &'a str, label: &'a str, page: Page, current: Page) -> Element<'a, Message> {
    let active = page == current;
    button(
        row![
            text(icon).size(21).width(24),
            text(label)
                .size(14)
                .font(if active { bold_font() } else { default_font() })
        ]
        .spacing(11)
        .align_y(alignment::Vertical::Center),
    )
    .padding([12, 15])
    .width(Length::Fill)
    .on_press(Message::Page(page))
    .style(move |_, status| {
        button_surface(
            if active { SAGE } else { Color::TRANSPARENT },
            if active { Color::WHITE } else { MUTED },
            status,
        )
    })
    .into()
}

fn segment(label: &str, composer: Composer, current: Composer) -> Element<'_, Message> {
    let active = composer == current;
    button(
        text(label)
            .size(14)
            .font(if active { bold_font() } else { default_font() }),
    )
    .padding([10, 15])
    .on_press(Message::Composer(composer))
    .style(move |_, status| {
        let mut style = button_surface(
            if active {
                Color::WHITE
            } else {
                Color::TRANSPARENT
            },
            if active { SAGE } else { MUTED },
            status,
        );
        style.border.radius = 10.0.into();
        if active {
            style.border.color = LINE;
            style.border.width = 1.0;
        }
        style
    })
    .into()
}

fn field_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Color::WHITE.into(),
        border: Border {
            color: if focused { SAGE } else { LINE },
            width: if focused { 1.5 } else { 1.0 },
            radius: 12.0.into(),
        },
        icon: MUTED,
        placeholder: MUTED,
        value: INK,
        selection: Color::from_rgb8(210, 227, 212),
    }
}
fn editor_style(_theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let focused = matches!(status, text_editor::Status::Focused { .. });
    text_editor::Style {
        background: BACKGROUND.into(),
        border: Border {
            color: if focused { SAGE } else { LINE },
            width: if focused { 1.5 } else { 1.0 },
            radius: 14.0.into(),
        },
        placeholder: MUTED,
        value: INK,
        selection: Color::from_rgb8(210, 227, 212),
    }
}
fn select_style(_theme: &Theme, _status: pick_list::Status) -> pick_list::Style {
    pick_list::Style {
        text_color: INK,
        placeholder_color: MUTED,
        handle_color: MUTED,
        background: Color::WHITE.into(),
        border: Border {
            color: LINE,
            width: 1.0,
            radius: 12.0.into(),
        },
    }
}
fn progress_style(_theme: &Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: LINE.into(),
        bar: SAGE.into(),
        border: Border {
            radius: 5.0.into(),
            ..Border::default()
        },
    }
}

fn scroll_style(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let mut style = scrollable::default(theme, status);
    style.container = tinted_surface(BACKGROUND, 0.0);
    style.vertical_rail.background = None;
    style.horizontal_rail.background = None;
    style.vertical_rail.scroller.background = Color::from_rgb8(193, 207, 193).into();
    style.horizontal_rail.scroller.background = Color::from_rgb8(193, 207, 193).into();
    style
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: Size::new(760.0, 540.0),
        position: window::Position::Centered,
        visible: false,
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

// Use logical work-area dimensions, not physical pixels. Allow small screens to
// shrink below our usual minimum instead of pushing controls past the taskbar.
fn fitted_size(preferred: Size, area: Size) -> Size {
    Size::new(
        preferred.width.min((area.width - 48.0).max(1.0)),
        preferred.height.min((area.height - 64.0).max(1.0)),
    )
}

fn monitor_area(id: window::Id) -> Task<Option<WorkArea>> {
    window::monitor_size(id).map(|size| {
        size.map(|size| WorkArea {
            origin: Point::ORIGIN,
            size,
        })
    })
}

fn work_area(id: window::Id) -> Task<Option<WorkArea>> {
    #[cfg(windows)]
    {
        window::run(id, |handle| {
            use raw_window_handle::RawWindowHandle;
            use windows_sys::Win32::{
                Graphics::Gdi::{
                    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
                },
                UI::HiDpi::GetDpiForWindow,
            };
            let RawWindowHandle::Win32(handle) = handle.window_handle().ok()?.as_raw() else {
                return None;
            };
            let hwnd = handle.hwnd.get() as windows_sys::Win32::Foundation::HWND;
            // Both APIs read geometry from our own Iced window. No external app
            // input or global DPI override is involved.
            unsafe {
                let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut info: MONITORINFO = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(monitor, &mut info) == 0 {
                    return None;
                }
                let dpi = GetDpiForWindow(hwnd);
                let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
                Some(WorkArea {
                    origin: Point::new(
                        info.rcWork.left as f32 / scale,
                        info.rcWork.top as f32 / scale,
                    ),
                    size: Size::new(
                        (info.rcWork.right - info.rcWork.left) as f32 / scale,
                        (info.rcWork.bottom - info.rcWork.top) as f32 / scale,
                    ),
                })
            }
        })
        .then(move |area| {
            if area.is_some() {
                Task::done(area)
            } else {
                monitor_area(id)
            }
        })
    }
    #[cfg(not(windows))]
    {
        monitor_area(id)
    }
}

fn fraction(done: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (done as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

fn remaining_time(bytes: u64, rate: f64) -> String {
    if !rate.is_finite() || rate <= 0.0 {
        return "正在估算剩余时间".into();
    }
    let seconds = (bytes as f64 / rate).ceil() as u64;
    if seconds < 60 {
        format!("约 {seconds} 秒")
    } else if seconds < 3600 {
        format!("约 {} 分 {} 秒", seconds / 60, seconds % 60)
    } else {
        format!("约 {} 小时 {} 分", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn local_addresses(port: u16) -> Vec<String> {
    let mut addresses: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| match interface.ip() {
            std::net::IpAddr::V4(ip)
                if !ip.is_loopback()
                    && (ip.is_private()
                        || ip.is_link_local()
                        || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))) =>
            {
                Some(format!("http://{ip}:{port}"))
            }
            _ => None,
        })
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn bytes(value: u64) -> String {
    if value >= 1024 * 1024 * 1024 {
        format!("{:.2} GiB", value as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if value >= 1024 * 1024 {
        format!("{:.1} MiB", value as f64 / (1024.0 * 1024.0))
    } else if value >= 1024 {
        format!("{:.1} KiB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

fn excerpt(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn test_status(value: &str) -> &'static str {
    match value {
        "installed" => "已确认安装",
        "passed" => "已确认通过",
        "failed" => "测试失败",
        "blocked" => "验收受阻",
        _ => "尚未验收",
    }
}

fn copy_new(source: &Path, target: &Path, expected: u64) -> Result<(), String> {
    let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|e| format!("无法创建文件，请选择新的文件名：{e}"))?;
    let result = io::copy(&mut input, &mut output).and_then(|size| {
        if size != expected {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "文件长度发生变化",
            ));
        }
        output.sync_all()
    });
    if result.is_err() {
        drop(output);
        let _ = fs::remove_file(target);
    }
    result.map_err(|e| e.to_string())
}

fn open_directory(directory: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(directory)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_window_fits_reported_windows_200_percent_work_area() {
        // Acceptance UI-01: 2560×1600 physical pixels at 200%, with the
        // Windows taskbar already removed from this logical work area.
        let area = Size::new(1280.0, 752.0);
        let size = fitted_size(Size::new(1120.0, 760.0), area);
        assert!(size.width + 32.0 <= area.width);
        assert!(size.height + 48.0 <= area.height);
        assert!(
            size.width >= 960.0,
            "normal 200% display keeps the side navigation"
        );
    }

    #[test]
    fn compact_work_area_can_shrink_below_usual_minimum() {
        let area = Size::new(640.0, 480.0);
        let size = fitted_size(Size::new(1120.0, 760.0), area);
        assert!(size.width < 620.0);
        assert!(size.height < 420.0);
        assert!(size.width > 0.0 && size.height > 0.0);
        assert!(size.width + 32.0 <= area.width);
        assert!(size.height + 48.0 <= area.height);
    }

    #[test]
    fn measured_rate_needs_confirmed_progress_and_expires_when_stalled() {
        let start = Instant::now();
        let mut sample = TransferSample {
            at: start,
            bytes: 0,
            rate: None,
            advanced: start,
        };
        assert_eq!(sample.rate_at(start), None);
        sample.observe(1024 * 1024, start + Duration::from_secs(1));
        assert_eq!(
            sample.rate_at(start + Duration::from_secs(1)),
            Some(1024.0 * 1024.0)
        );
        sample.observe(1024 * 1024, start + Duration::from_secs(3));
        assert_eq!(sample.rate_at(start + Duration::from_secs(5)), None);
        sample.observe(0, start + Duration::from_secs(6));
        assert_eq!(sample.rate_at(start + Duration::from_secs(6)), None);
        sample.observe(512 * 1024, start + Duration::from_secs(7));
        assert_eq!(
            sample.rate_at(start + Duration::from_secs(7)),
            Some(512.0 * 1024.0)
        );
    }

    #[test]
    fn saving_original_never_overwrites_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("原图.bin");
        let target = directory.path().join("已有.bin");
        fs::write(&source, [0u8, 255, 3, 8]).unwrap();
        fs::write(&target, b"keep").unwrap();
        assert!(copy_new(&source, &target, 4).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        let fresh = directory.path().join("新的.bin");
        copy_new(&source, &fresh, 4).unwrap();
        assert_eq!(fs::read(source).unwrap(), fs::read(fresh).unwrap());
    }

    #[test]
    fn failed_save_removes_only_the_file_it_created() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("short");
        let target = directory.path().join("partial");
        fs::write(&source, b"abc").unwrap();
        assert!(copy_new(&source, &target, 8).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read(source).unwrap(), b"abc");
    }
}
