//! 整理服务器返回的文件夹列表，让不同邮箱的目录保持一致：
//!
//! - 特殊文件夹（收件箱、草稿箱、已发送、归档、垃圾邮件、已删除）使用统一的中文名称，
//!   按固定顺序排在最前；
//! - 其他文件夹按层级缩进，只显示自己的名称；
//! - 服务商内部使用的系统文件夹（如 Outlook 的“同步问题”、Outbox、Notes）以及
//!   没有可打开子文件夹的分组标记为隐藏，界面可按需显示。

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::mail::MailFolder;

use super::connection::host_in;

/// 特殊文件夹的固定顺序与统一名称。
const SPECIAL: [(&str, &str); 6] = [
    ("inbox", "收件箱"),
    ("drafts", "草稿箱"),
    ("sent", "已发送"),
    ("archive", "归档"),
    ("junk", "垃圾邮件"),
    ("trash", "已删除"),
];

/// 服务器没有声明 SPECIAL-USE 属性时，按常见名称识别特殊文件夹（小写比较）。
pub(super) fn special_names(kind: &str) -> &'static [&'static str] {
    match kind {
        "drafts" => &["drafts", "draft", "inbox.drafts", "草稿", "草稿箱"],
        "sent" => &[
            "sent",
            "sent items",
            "sent messages",
            "sent mail",
            "inbox.sent",
            "已发送",
            "已发送邮件",
        ],
        "archive" => &["archive", "archives", "inbox.archive", "归档", "存档"],
        "junk" => &[
            "junk",
            "spam",
            "junk e-mail",
            "junk email",
            "bulk mail",
            "inbox.junk",
            "inbox.spam",
            "垃圾邮件",
            "垃圾箱",
        ],
        "trash" => &[
            "trash",
            "deleted",
            "deleted items",
            "deleted messages",
            "inbox.trash",
            "已删除",
            "已删除邮件",
        ],
        _ => &[],
    }
}

/// 服务商内部使用、对收发邮件没有意义的顶层文件夹（含其子文件夹）。
fn system_folders(incoming_host: &str) -> &'static [&'static str] {
    if host_in(incoming_host, &["office365.com", "outlook.com"]) {
        &[
            "sync issues",
            "同步问题",
            "outbox",
            "发件箱",
            "notes",
            "便笺",
            "journal",
            "日记",
            "tasks",
            "任务",
            "calendar",
            "日历",
            "contacts",
            "联系人",
            "conversation history",
            "对话历史记录",
            "rss feeds",
            "rss 源",
            "rss 订阅",
        ]
    } else if host_in(incoming_host, &["me.com", "icloud.com"]) {
        &["notes"]
    } else {
        &[]
    }
}

fn segments(folder: &MailFolder) -> Vec<String> {
    match folder
        .delimiter
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        Some(delimiter) => folder
            .display_name
            .split(delimiter)
            .map(str::to_string)
            .collect(),
        None => vec![folder.display_name.clone()],
    }
}

pub(super) fn arrange(mut folders: Vec<MailFolder>, incoming_host: &str) -> Vec<MailFolder> {
    let paths: Vec<Vec<String>> = folders.iter().map(segments).collect();
    let system = system_folders(incoming_host);

    // 1. 识别特殊文件夹：先认属性，没有时按顶层名称；同类只保留第一个。
    for (kind, _) in SPECIAL.iter().skip(1) {
        if folders.iter().any(|folder| folder.kind == *kind) {
            continue;
        }
        if let Some(index) = folders.iter().position(|folder| {
            folder.selectable
                && folder.kind == "custom"
                && special_names(kind).contains(&folder.display_name.to_lowercase().as_str())
        }) {
            folders[index].kind = (*kind).to_string();
        }
    }
    let mut seen = HashSet::new();
    for folder in &mut folders {
        if folder.kind != "custom" && !seen.insert(folder.kind.clone()) {
            folder.kind = "custom".into();
        }
    }

    // 2. 系统文件夹及其子文件夹隐藏。
    for (folder, path) in folders.iter_mut().zip(&paths) {
        folder.hidden = folder.kind == "custom"
            && path
                .first()
                .is_some_and(|top| system.contains(&top.to_lowercase().as_str()));
    }

    // 3. 普通文件夹组成树：只显示自己的名称，按层级缩进。
    //    父级是特殊文件夹（如 INBOX/工作）时，子文件夹直接放在顶层。
    let by_path: HashMap<Vec<String>, usize> = paths
        .iter()
        .enumerate()
        .map(|(index, path)| (path.clone(), index))
        .collect();
    let mut children: BTreeMap<Vec<String>, Vec<usize>> = BTreeMap::new();
    let mut roots = Vec::new();
    for (index, folder) in folders.iter().enumerate() {
        if folder.kind != "custom" {
            continue;
        }
        let path = &paths[index];
        let parent = (1..path.len())
            .rev()
            .map(|length| path[..length].to_vec())
            .find(|prefix| {
                by_path
                    .get(prefix)
                    .is_some_and(|&parent| folders[parent].kind == "custom")
            });
        match parent {
            Some(parent) => children.entry(parent).or_default().push(index),
            None => roots.push(index),
        }
    }
    let leaf = |index: usize| paths[index].last().cloned().unwrap_or_default();
    let sort = |items: &mut Vec<usize>| {
        items.sort_by(|&a, &b| {
            leaf(a)
                .to_lowercase()
                .cmp(&leaf(b).to_lowercase())
                .then_with(|| folders[a].name.cmp(&folders[b].name))
        });
    };
    sort(&mut roots);
    for list in children.values_mut() {
        sort(list);
    }

    // 只有子文件夹的分组（不可打开）在没有可见子文件夹时隐藏。
    fn has_visible(
        index: usize,
        folders: &[MailFolder],
        paths: &[Vec<String>],
        children: &BTreeMap<Vec<String>, Vec<usize>>,
    ) -> bool {
        (folders[index].selectable && !folders[index].hidden)
            || children.get(&paths[index]).is_some_and(|list| {
                list.iter()
                    .any(|&child| has_visible(child, folders, paths, children))
            })
    }

    let mut order = Vec::with_capacity(folders.len());
    for (kind, _) in SPECIAL {
        order.extend(folders.iter().position(|folder| folder.kind == kind));
    }
    let mut depth = vec![0u8; folders.len()];
    let mut stack: Vec<(usize, u8)> = roots.iter().rev().map(|&index| (index, 0)).collect();
    while let Some((index, level)) = stack.pop() {
        depth[index] = level;
        order.push(index);
        if let Some(list) = children.get(&paths[index]) {
            stack.extend(
                list.iter()
                    .rev()
                    .map(|&child| (child, level.saturating_add(1))),
            );
        }
    }
    let containers: Vec<bool> = (0..folders.len())
        .map(|index| !folders[index].selectable && !has_visible(index, &folders, &paths, &children))
        .collect();

    let mut arranged = Vec::with_capacity(folders.len());
    let mut taken = vec![false; folders.len()];
    for (position, index) in order.into_iter().enumerate() {
        taken[index] = true;
        let mut folder = folders[index].clone();
        folder.order = position as u32;
        folder.depth = depth[index];
        folder.hidden |= containers[index];
        if let Some((_, name)) = SPECIAL.iter().find(|(kind, _)| folder.kind == *kind) {
            folder.display_name = (*name).to_string();
        } else {
            folder.display_name = leaf(index);
        }
        arranged.push(folder);
    }
    // 理论上不会遗漏；保险起见把未排到的文件夹放在最后。
    for (index, folder) in folders.into_iter().enumerate() {
        if !taken[index] {
            let position = arranged.len() as u32;
            arranged.push(MailFolder {
                order: position,
                ..folder
            });
        }
    }
    arranged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, kind: &str) -> MailFolder {
        MailFolder {
            name: name.into(),
            display_name: name.into(),
            selectable: true,
            kind: kind.into(),
            delimiter: Some("/".into()),
            ..MailFolder::default()
        }
    }

    fn view(folders: &[MailFolder]) -> Vec<String> {
        folders
            .iter()
            .filter(|folder| !folder.hidden)
            .map(|folder| {
                format!(
                    "{}{}",
                    "  ".repeat(folder.depth as usize),
                    folder.display_name
                )
            })
            .collect()
    }

    #[test]
    fn outlook_system_folders_are_hidden_and_special_folders_are_unified() {
        let folders = vec![
            folder("Archive", "archive"),
            folder("Deleted", "trash"),
            folder("Drafts", "drafts"),
            folder("INBOX", "inbox"),
            folder("Junk", "junk"),
            folder("Notes", "custom"),
            folder("Outbox", "custom"),
            folder("Sent", "sent"),
            folder("同步问题", "custom"),
            folder("同步问题/冲突", "custom"),
            folder("同步问题/服务器故障", "custom"),
            folder("同步问题/本地故障", "custom"),
            folder("项目", "custom"),
            folder("项目/2026", "custom"),
            folder("Bills", "custom"),
        ];
        let arranged = arrange(folders, "outlook.office365.com");
        assert_eq!(
            view(&arranged),
            [
                "收件箱",
                "草稿箱",
                "已发送",
                "归档",
                "垃圾邮件",
                "已删除",
                "Bills",
                "项目",
                "  2026"
            ]
        );
        let hidden: Vec<_> = arranged
            .iter()
            .filter(|folder| folder.hidden)
            .map(|folder| folder.name.as_str())
            .collect();
        assert_eq!(
            hidden,
            [
                "Notes",
                "Outbox",
                "同步问题",
                "同步问题/冲突",
                "同步问题/服务器故障",
                "同步问题/本地故障"
            ]
        );
        // 原始名称保留，用于和服务器通信。
        assert!(
            arranged
                .iter()
                .any(|folder| folder.name == "Deleted" && folder.display_name == "已删除")
        );
        let orders: Vec<_> = arranged.iter().map(|folder| folder.order).collect();
        assert_eq!(orders, (0..arranged.len() as u32).collect::<Vec<_>>());
    }

    #[test]
    fn same_names_on_other_servers_are_not_hidden() {
        let arranged = arrange(
            vec![
                folder("INBOX", "inbox"),
                folder("Notes", "custom"),
                folder("Tasks", "custom"),
            ],
            "imap.example.com",
        );
        assert_eq!(view(&arranged), ["收件箱", "Notes", "Tasks"]);
    }

    #[test]
    fn special_folders_without_attributes_are_recognised_by_name_once() {
        let mut netease = vec![
            folder("INBOX", "inbox"),
            folder("草稿箱", "custom"),
            folder("已发送", "custom"),
            folder("已删除", "custom"),
            folder("垃圾邮件", "custom"),
            folder("病毒文件夹", "custom"),
            folder("Trash", "custom"),
        ];
        netease[2].kind = "sent".into();
        let arranged = arrange(netease, "imap.163.com");
        assert_eq!(
            view(&arranged),
            [
                "收件箱",
                "草稿箱",
                "已发送",
                "垃圾邮件",
                "已删除",
                "Trash",
                "病毒文件夹"
            ]
        );
        assert_eq!(arranged[4].name, "已删除");
        assert_eq!(arranged[5].kind, "custom", "only one trash folder");
    }

    #[test]
    fn containers_show_only_when_they_have_visible_children() {
        let mut container = folder("[Gmail]", "custom");
        container.selectable = false;
        let mut other = folder("其他文件夹", "custom");
        other.selectable = false;
        let arranged = arrange(
            vec![
                folder("INBOX", "inbox"),
                container,
                folder("[Gmail]/Sent Mail", "sent"),
                folder("[Gmail]/Trash", "trash"),
                other,
                folder("其他文件夹/订阅", "custom"),
                folder("INBOX/工作", "custom"),
            ],
            "imap.gmail.com",
        );
        assert_eq!(
            view(&arranged),
            ["收件箱", "已发送", "已删除", "其他文件夹", "  订阅", "工作"]
        );
        assert!(
            arranged
                .iter()
                .any(|folder| folder.name == "[Gmail]" && folder.hidden)
        );
    }

    #[test]
    fn folders_without_delimiter_are_flat() {
        let mut flat = folder("A/B", "custom");
        flat.delimiter = None;
        let arranged = arrange(vec![flat], "imap.example.com");
        assert_eq!(view(&arranged), ["A/B"]);
    }
}
