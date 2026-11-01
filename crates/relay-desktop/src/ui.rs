use crate::{
    preferences::{Preferences, QueuedFile},
    tray::TrayAction,
    Host, Signal, SIGNALS,
};
use iced::widget::{
    button, checkbox, column, container, pick_list, progress_bar, qr_code, row, rule, scrollable,
    text, text_editor, text_input, Space,
};
use iced::{
    alignment, border, clipboard, event, window, Border, Color, Element, Font, Length,
    Subscription, Task, Theme,
};
use relay_core::{Invite, Item, ItemPage, NewFeedback, NewText, PatchItem, Snapshot, OWNER_ID};
use std::{
    collections::{HashSet, VecDeque},
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
    Opened,
    CloseRequested(window::Id),
    Dropped(PathBuf),
    Page(Page),
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
        let mut notice = tray_notice.unwrap_or_else(|| {
            "拖入文件，或写一段文字。内容会出现在已批准设备的共同收件区。".into()
        });
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
                _ => None,
            }),
        ];
        if self.window.is_some() && !self.exiting {
            subscriptions.push(iced::time::every(Duration::from_secs(10)).map(|_| Message::Tick));
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
            Message::Opened => Task::none(),
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
                self.page = page;
                if page == Page::Devices && self.invite.is_none() {
                    Task::done(Message::NewInvite)
                } else {
                    Task::none()
                }
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
        let devices = self
            .snapshot
            .as_ref()
            .map(|s| s.devices.iter().filter(|d| !d.revoked).count())
            .unwrap_or(0);
        let pending = self
            .snapshot
            .as_ref()
            .map(|s| {
                s.pair_requests
                    .iter()
                    .filter(|p| p.status == "pending")
                    .count()
            })
            .unwrap_or(0);
        let brand = column![
            text("随手传").size(30).font(bold_font()).color(ORANGE),
            text("手机与电脑，近一点").size(13).color(MUTED)
        ]
        .spacing(3);
        let tabs = row![
            nav("收发内容", Page::Feed, self.page),
            nav(
                if pending > 0 {
                    "连接设备 · 待批准"
                } else {
                    "连接设备"
                },
                Page::Devices,
                self.page
            ),
            nav("传输任务", Page::Transfers, self.page),
        ]
        .spacing(8);
        let header = row![
            brand,
            Space::new().width(Length::Fill),
            column![
                text(format!("{devices} 台已批准设备")).size(13),
                text(if self.tray_enabled {
                    "关窗收起到托盘"
                } else {
                    "托盘不可用 · 关窗退出服务"
                })
                .size(11)
            ]
            .spacing(4),
            button("退出").on_press(Message::Quit).style(button::text)
        ]
        .spacing(16)
        .align_y(alignment::Vertical::Center);
        let status =
            container(
                text(&self.notice)
                    .size(13)
                    .color(if self.error { RED } else { MUTED }),
            )
            .padding([10, 14])
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(
                    if self.error {
                        Color::from_rgb8(255, 239, 235)
                    } else {
                        Color::from_rgb8(247, 244, 238)
                    }
                    .into(),
                ),
                border: Border {
                    radius: 10.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            });
        let content = match self.page {
            Page::Feed => self.feed(),
            Page::Devices => self.devices(),
            Page::Transfers => self.transfers(),
        };
        container(
            column![header, tabs, status, content]
                .spacing(16)
                .padding(24),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn feed(&self) -> Element<'_, Message> {
        let mut compose = column![row![
            text_input("项目（可选，记住上次选择）", &self.project)
                .on_input(Message::Project)
                .width(Length::Fill),
            button("＋ 选择文件")
                .on_press(Message::ChooseFiles)
                .padding([10, 16])
        ]
        .spacing(12),]
        .spacing(10);
        if let Some(build) = &self.feedback_build {
            compose = compose.push(
                row![
                    text(format!("反馈给：{}", build.label))
                        .size(14)
                        .color(ORANGE),
                    Space::new().width(Length::Fill),
                    button("改为普通文字")
                        .on_press(Message::Build(None))
                        .style(button::text)
                ]
                .spacing(8),
            );
        }
        compose = compose.push(
            text_editor(&self.editor)
                .placeholder(if self.feedback_build.is_some() {
                    "哪里不符合预期？写下复现步骤，然后在下方选择截图或录屏作为附件。"
                } else {
                    "粘贴文字、链接、日志片段…"
                })
                .height(112)
                .padding(12)
                .size(15)
                .on_action(Message::Editor),
        );
        let send_label = if self.sending {
            "发送中…"
        } else if self.feedback_build.is_some() {
            "发送反馈"
        } else {
            "发送文字"
        };
        let mut send_button = button(send_label).padding([9, 24]);
        if !self.sending && !self.exiting {
            send_button = send_button.on_press(Message::Send);
        }
        compose = compose.push(
            row![
                text(if self.feedback_build.is_some() {
                    format!("已选择 {} 个反馈附件", self.attachments.len())
                } else {
                    "原图与文件保持原始字节，不转码。".into()
                })
                .size(12)
                .color(MUTED),
                Space::new().width(Length::Fill),
                send_button
            ]
            .align_y(alignment::Vertical::Center),
        );

        let query = self.query.to_lowercase();
        let visible = self.items.iter().filter(|item| {
            let project_matches = match &self.filter {
                ProjectFilter::All => true,
                ProjectFilter::Unassigned => item.project.is_empty(),
                ProjectFilter::Named(project) => item.project == *project,
            };
            project_matches
                && (query.is_empty()
                    || format!("{} {} {} {}", item.name, item.text, item.project, item.note)
                        .to_lowercase()
                        .contains(&query))
        });
        let mut list = column![].spacing(10);
        let mut count = 0;
        for item in visible {
            list = list.push(self.item_card(item));
            count += 1;
        }
        if count == 0 {
            list = list.push(container(column![text("准备好接住你的内容").size(21).font(bold_font()), text("拖入文件，或在手机分享给随手传。\n第一次使用，请先到「连接设备」配对。").size(14).color(MUTED)].spacing(12)).padding(24));
        }
        if self.next_cursor.is_some() {
            let mut more = button(if self.history_loading {
                "加载中…"
            } else {
                "加载更早的内容"
            })
            .style(button::secondary)
            .padding(12);
            if !self.history_loading {
                more = more.on_press(Message::More);
            }
            list = list.push(more);
        }
        let mut projects: Vec<String> = self
            .items
            .iter()
            .filter(|item| !item.project.is_empty())
            .map(|item| item.project.clone())
            .collect();
        projects.sort();
        projects.dedup();
        let filters: Vec<_> = [ProjectFilter::All, ProjectFilter::Unassigned]
            .into_iter()
            .chain(projects.into_iter().map(ProjectFilter::Named))
            .collect();
        let heading = row![
            text(format!("最近内容 · 已加载 {} 条", self.items.len()))
                .size(15)
                .font(bold_font()),
            Space::new().width(Length::Fill),
            pick_list(filters, Some(self.filter.clone()), Message::Filter).width(160),
            text_input("搜索已加载内容", &self.query)
                .on_input(Message::Query)
                .width(210),
            button("刷新").on_press(Message::Reload).style(button::text)
        ]
        .spacing(10)
        .align_y(alignment::Vertical::Center);
        column![
            container(compose).padding(16).style(card),
            heading,
            scrollable(list).height(Length::Fill)
        ]
        .spacing(16)
        .height(Length::Fill)
        .into()
    }

    fn item_card<'a>(&'a self, item: &'a Item) -> Element<'a, Message> {
        let is_file = item.kind == "file";
        let is_apk = is_file && item.name.to_lowercase().ends_with(".apk");
        let kind = if is_apk {
            "APK"
        } else if item.mime.starts_with("image/") {
            "图片"
        } else if item.mime.starts_with("video/") {
            "录屏 / 视频"
        } else if is_file {
            "文件"
        } else if item.kind == "feedback" {
            "测试反馈"
        } else {
            "文字"
        };
        let title = if is_file {
            item.name.clone()
        } else {
            excerpt(&item.text, 100)
        };
        let project = if item.project.is_empty() {
            "未分项目"
        } else {
            &item.project
        };
        let mut body = column![
            row![
                text(kind).size(12).color(ORANGE).font(bold_font()),
                text(project).size(12).color(MUTED),
                Space::new().width(Length::Fill),
                text(if item.device_id == OWNER_ID {
                    "来自电脑"
                } else {
                    "来自设备"
                })
                .size(12)
                .color(MUTED)
            ]
            .spacing(12),
            text(title)
                .size(16)
                .font(if is_file { bold_font() } else { default_font() }),
        ]
        .spacing(8);
        let mut actions = row![].spacing(8).align_y(alignment::Vertical::Center);
        if is_file {
            actions = actions
                .push(text(bytes(item.size)).size(12).color(MUTED))
                .push(
                    button("另存为")
                        .on_press(Message::SaveFile(item.id.clone()))
                        .style(button::secondary),
                );
            if is_apk {
                actions = actions
                    .push(
                        button("关联反馈")
                            .on_press(Message::Build(Some(BuildChoice {
                                id: item.id.clone(),
                                label: item.name.clone(),
                            })))
                            .style(button::text),
                    )
                    .push(text(test_status(&item.test_status)).size(12).color(MUTED));
            }
            if self
                .feedback_build
                .as_ref()
                .is_some_and(|b| b.id != item.id)
            {
                let id = item.id.clone();
                actions = actions.push(
                    checkbox(self.attachments.contains(&item.id))
                        .label("反馈附件")
                        .on_toggle(move |selected| Message::Attachment(id.clone(), selected)),
                );
            }
        } else {
            actions = actions.push(
                button("复制全文")
                    .on_press(Message::Copy(item.text.clone()))
                    .style(button::secondary),
            );
            if item.kind == "feedback" {
                actions = actions.push(
                    button("导出给 Codex")
                        .on_press(Message::Export(item.id.clone()))
                        .style(button::text),
                );
            }
        }
        let expanded = self.selected_item.as_deref() == Some(item.id.as_str());
        actions = actions.push(Space::new().width(Length::Fill)).push(
            button(if expanded { "收起" } else { "详情" })
                .on_press(Message::Details(if expanded {
                    None
                } else {
                    Some(item.id.clone())
                }))
                .style(button::text),
        );
        body = body.push(actions);
        if expanded {
            body = body.push(rule::horizontal(1));
            if !item.text.is_empty() {
                body = body.push(text(&item.text).size(14));
            }
            if !item.sha256.is_empty() {
                body = body.push(
                    text(format!("SHA-256  {}", item.sha256))
                        .size(12)
                        .wrapping(text::Wrapping::Glyph),
                );
            }
            body = body.push(text(format!("记录 {}", item.id)).size(11).color(MUTED));
            if let Some(apk) = &item.apk {
                body = body.push(
                    text(format!(
                        "包名：{}    版本：{}",
                        apk.package_name.as_deref().unwrap_or("未知"),
                        apk.version_name.as_deref().unwrap_or("未知")
                    ))
                    .size(12)
                    .color(MUTED),
                );
            }
            if is_apk {
                body = body.push(
                    text("手动确认测试状态；下载不代表已安装或测试通过。")
                        .size(12)
                        .color(MUTED),
                );
                let mut marks = row![].spacing(6);
                for (value, label) in [
                    ("unverified", "未验收"),
                    ("installed", "已安装"),
                    ("passed", "通过"),
                    ("failed", "失败"),
                    ("blocked", "受阻"),
                ] {
                    marks = marks.push(
                        button(label)
                            .on_press(Message::Mark(item.id.clone(), value.into()))
                            .style(button::secondary),
                    );
                }
                body = body.push(marks);
            }
        }
        container(body)
            .padding(16)
            .width(Length::Fill)
            .style(card)
            .into()
    }

    fn devices(&self) -> Element<'_, Message> {
        let mut invite = column![
            text("连接你的手机").size(23).font(bold_font()),
            text("两台设备连接可互访的局域网。打开新版手机 App 扫码，或输入地址和六位连接码。")
                .size(14)
                .color(MUTED)
        ]
        .spacing(12);
        if self.addresses.is_empty() {
            invite = invite.push(
                text(
                    "没有发现可用的局域网 IPv4 地址。连接 Wi-Fi / 网线后，点击生成连接码重新检测。",
                )
                .color(RED),
            );
        } else {
            invite = invite.push(
                pick_list(
                    self.addresses.clone(),
                    self.selected_address.clone(),
                    Message::Address,
                )
                .width(Length::Fill),
            );
        }
        let mut invite_button = button(if self.invite_loading {
            "正在生成…"
        } else {
            "生成 / 更新连接码"
        })
        .padding([10, 16]);
        if !self.invite_loading {
            invite_button = invite_button.on_press(Message::NewInvite);
        }
        invite = invite.push(invite_button);
        if let Some(value) = &self.invite {
            let remaining = (value.expires_at - unix_millis()).max(0) / 1000;
            if remaining > 0 {
                let mut code = column![
                    text(&value.code).size(38).font(bold_font()).color(ORANGE),
                    text(format!("约 {remaining} 秒内有效，电脑批准后连接"))
                        .size(12)
                        .color(MUTED)
                ]
                .spacing(8);
                if let Some(link) = self.pairing_url() {
                    code = code.push(
                        button("复制配对链接")
                            .on_press(Message::Copy(link))
                            .style(button::secondary),
                    );
                }
                if let Some(qr) = &self.qr {
                    invite = invite.push(
                        row![qr_code(qr).cell_size(4), code]
                            .spacing(24)
                            .align_y(alignment::Vertical::Center),
                    );
                } else {
                    invite = invite.push(code);
                }
            } else {
                invite = invite.push(text("连接码已过期，请重新生成。").color(MUTED));
            }
        }
        invite = invite.push(
            text("当前使用局域网 HTTP，内容未加密。只在可信网络中使用；不要设置公网转发。")
                .size(12)
                .color(MUTED),
        );
        let mut content = column![container(invite).padding(20).style(card)].spacing(16);
        if let Some(snapshot) = &self.snapshot {
            let pending: Vec<_> = snapshot
                .pair_requests
                .iter()
                .filter(|p| p.status == "pending" && p.expires_at > unix_millis())
                .collect();
            if !pending.is_empty() {
                content = content.push(text("等待你批准").size(17).font(bold_font()));
                for request in pending {
                    content = content.push(
                        container(
                            row![
                                text(&request.name).width(Length::Fill),
                                button("拒绝")
                                    .on_press(Message::Pair(request.id.clone(), false))
                                    .style(button::secondary),
                                button("批准连接")
                                    .on_press(Message::Pair(request.id.clone(), true))
                            ]
                            .spacing(10)
                            .align_y(alignment::Vertical::Center),
                        )
                        .padding(14)
                        .style(card),
                    );
                }
            }
            content = content.push(text("已批准设备").size(17).font(bold_font()));
            for device in snapshot.devices.iter().filter(|d| !d.revoked) {
                let mut actions = row![text(&device.name).width(Length::Fill)]
                    .spacing(10)
                    .align_y(alignment::Vertical::Center);
                if self.revoke_prompt.as_deref() == Some(&device.id) {
                    actions = actions
                        .push(
                            button("取消")
                                .on_press(Message::RevokePrompt(None))
                                .style(button::text),
                        )
                        .push(
                            button("确认撤销")
                                .on_press(Message::Revoke(device.id.clone()))
                                .style(button::danger),
                        );
                } else {
                    actions = actions.push(
                        button("撤销连接")
                            .on_press(Message::RevokePrompt(Some(device.id.clone())))
                            .style(button::secondary),
                    );
                }
                content = content.push(container(actions).padding(14).style(card));
            }
            content = content.push(
                text("所有已批准设备共享本电脑的内容区。项目名称只用于整理。")
                    .size(12)
                    .color(MUTED),
            );
        }
        scrollable(content).height(Length::Fill).into()
    }

    fn transfers(&self) -> Element<'_, Message> {
        let mut content = column![
            text("本机导入队列").size(21).font(bold_font()),
            text("文件先导入并校验到电脑收件区，手机随后下载。这里的进度不是手机接收进度。")
                .size(13)
                .color(MUTED)
        ]
        .spacing(12);
        if let Some(progress) = &self.importing {
            let fraction = if progress.total > 0 {
                progress.done as f32 / progress.total as f32
            } else {
                0.0
            };
            content = content.push(
                container(
                    column![
                        text(format!("导入 / 校验：{}", progress.name)),
                        progress_bar(0.0..=1.0, fraction),
                        text(format!(
                            "{} / {}",
                            bytes(progress.done),
                            bytes(progress.total)
                        ))
                        .size(12)
                    ]
                    .spacing(8),
                )
                .padding(16)
                .style(card),
            );
        }
        if !self.queue.is_empty() {
            content = content.push(
                button(if self.queue_paused {
                    "继续本机队列"
                } else {
                    "暂停后续文件"
                })
                .on_press(Message::ToggleQueue)
                .style(button::secondary),
            );
            for (index, queued) in self.queue.iter().enumerate() {
                let mut line = row![
                    text(
                        queued
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    )
                    .width(Length::Fill),
                    text(&queued.project).size(12).color(MUTED)
                ]
                .spacing(10);
                if !(index == 0 && self.importing.is_some()) {
                    line = line.push(
                        button("移出队列")
                            .on_press(Message::RemoveQueued(index))
                            .style(button::text),
                    );
                }
                content = content.push(line);
            }
        } else {
            content = content.push(text("没有等待导入的本机文件。").size(14).color(MUTED));
        }
        if let Some(snapshot) = &self.snapshot {
            content = content
                .push(rule::horizontal(1))
                .push(text("设备传输任务").size(21).font(bold_font()));
            for upload in snapshot
                .uploads
                .iter()
                .filter(|u| u.status != "complete" && u.status != "cancelled")
            {
                let fraction = if upload.size > 0 {
                    upload.offset as f32 / upload.size as f32
                } else {
                    0.0
                };
                let mut line = column![
                    text(&upload.name).font(bold_font()),
                    progress_bar(0.0..=1.0, fraction),
                    text(format!(
                        "已确认 {} / {} · {}",
                        bytes(upload.offset),
                        bytes(upload.size),
                        if upload.status == "failed" {
                            "失败，请在原设备处理"
                        } else {
                            "等待或传输中"
                        }
                    ))
                    .size(12)
                    .color(MUTED)
                ]
                .spacing(8);
                line = line.push(
                    button("取消此传输")
                        .on_press(Message::CancelUpload(upload.id.clone()))
                        .style(button::secondary),
                );
                content = content.push(container(line).padding(16).style(card));
            }
            content = content.push(
                text("暂停、恢复和重选源文件在发送设备操作。此处显示服务器已保存的位置。")
                    .size(12)
                    .color(MUTED),
            );
            content = content.push(rule::horizontal(1)).push(
                text(format!(
                    "收件区：{} / {}",
                    bytes(snapshot.used_bytes),
                    bytes(snapshot.max_storage_bytes)
                ))
                .size(14),
            );
        }
        content = content.push(
            button("打开电脑数据目录")
                .on_press(Message::OpenInbox)
                .style(button::secondary),
        );
        content = content.push(
            text(format!("版本 {} · 协议 2", env!("CARGO_PKG_VERSION")))
                .size(12)
                .color(MUTED),
        );
        scrollable(content).height(Length::Fill).into()
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

const ORANGE: Color = Color::from_rgb(1.0, 0.416, 0.0);
const MUTED: Color = Color::from_rgb(0.43, 0.43, 0.42);
const RED: Color = Color::from_rgb(0.72, 0.20, 0.15);

pub fn theme() -> Theme {
    Theme::custom(
        "Warm Relay",
        iced::theme::Palette {
            background: Color::from_rgb8(250, 248, 244),
            text: Color::from_rgb8(38, 38, 35),
            primary: ORANGE,
            success: Color::from_rgb8(43, 120, 91),
            danger: RED,
            warning: Color::from_rgb8(174, 117, 18),
        },
    )
}

pub fn default_font() -> Font {
    #[cfg(windows)]
    {
        Font::with_name("Microsoft YaHei UI")
    }
    #[cfg(not(windows))]
    {
        Font::DEFAULT
    }
}

fn bold_font() -> Font {
    Font {
        weight: iced::font::Weight::Bold,
        ..default_font()
    }
}

fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::WHITE.into()),
        border: Border {
            color: Color::from_rgb8(232, 228, 220),
            width: 1.0,
            radius: border::Radius::from(14.0),
        },
        ..container::Style::default()
    }
}

fn nav(label: &str, page: Page, current: Page) -> Element<'_, Message> {
    button(text(label).size(15))
        .padding([10, 16])
        .on_press(Message::Page(page))
        .style(if page == current {
            button::primary
        } else {
            button::text
        })
        .into()
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: iced::Size::new(1040.0, 820.0),
        min_size: Some(iced::Size::new(760.0, 600.0)),
        exit_on_close_request: false,
        ..window::Settings::default()
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
