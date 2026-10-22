//! Eden's own entries of the item (file/folder) context menu.
//!
//! [`EdenMenuEntry::ALL`] is the single source of truth for the entries: the
//! item menu draws them in this order and the settings page lists them so
//! each one can be hidden. Hidden keys are persisted with the shell entry
//! preferences (`context_menu.json`, see [`shell_context_menu`]).

use crate::gui::windows::shell_context_menu;
use std::cell::RefCell;

/// Prefix of every Eden entry key. Shell entry keys start with `verb:` or
/// `label:`, so the two can never collide (checked by the tests).
#[cfg(test)]
pub const EDEN_KEY_PREFIX: &str = "eden:";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EdenMenuEntry {
    OpenInNewTab,
    OpenInSplitView,
    ManageTags,
    OpenWithDefault,
    Cut,
    Copy,
    CopyPath,
    Paste,
    Rename,
    Delete,
    Properties,
}

impl EdenMenuEntry {
    /// Every entry, in menu order.
    pub const ALL: [Self; 11] = [
        Self::OpenInNewTab,
        Self::OpenInSplitView,
        Self::ManageTags,
        Self::OpenWithDefault,
        Self::Cut,
        Self::Copy,
        Self::CopyPath,
        Self::Paste,
        Self::Rename,
        Self::Delete,
        Self::Properties,
    ];

    /// Stable persisted key. Never change an existing key: it would reset
    /// the user's choice.
    pub const fn key(self) -> &'static str {
        match self {
            Self::OpenInNewTab => "eden:open_in_new_tab",
            Self::OpenInSplitView => "eden:open_in_split_view",
            Self::ManageTags => "eden:manage_tags",
            Self::OpenWithDefault => "eden:open_with_default",
            Self::Cut => "eden:cut",
            Self::Copy => "eden:copy",
            Self::CopyPath => "eden:copy_path",
            Self::Paste => "eden:paste",
            Self::Rename => "eden:rename",
            Self::Delete => "eden:delete",
            Self::Properties => "eden:properties",
        }
    }

    /// i18n key of the label shown on the settings page. The menu uses the
    /// same label (the open entry switches to its plural form for several
    /// files).
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::OpenInNewTab => "inputs_newtab",
            Self::OpenInSplitView => "inputs_opensplit",
            Self::ManageTags => "tag_manage",
            Self::OpenWithDefault => "open_default_program",
            Self::Cut => "inputs_cut",
            Self::Copy => "inputs_copy",
            Self::CopyPath => "inputs_copy_path",
            Self::Paste => "inputs_paste",
            Self::Rename => "inputs_rename",
            Self::Delete => "inputs_delete",
            Self::Properties => "properties",
        }
    }

    /// Entries of the same group are drawn together; a separator is drawn
    /// between two visible entries of different groups.
    const fn group(self) -> u8 {
        match self {
            Self::OpenInNewTab
            | Self::OpenInSplitView
            | Self::ManageTags
            | Self::OpenWithDefault => 0,
            Self::Cut
            | Self::Copy
            | Self::CopyPath
            | Self::Paste
            | Self::Rename
            | Self::Delete
            | Self::Properties => 1,
        }
    }
}

/// One slot of the drawn Eden part of the item menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdenMenuSlot {
    Entry(EdenMenuEntry),
    Separator,
}

/// The visible entries in menu order with separators between groups. Since
/// separators are only inserted between two visible entries, there are never
/// leading, trailing or adjacent separators.
fn layout(is_hidden: impl Fn(EdenMenuEntry) -> bool) -> Vec<EdenMenuSlot> {
    let mut slots = Vec::with_capacity(EdenMenuEntry::ALL.len() + 1);
    let mut previous_group = None;
    for entry in EdenMenuEntry::ALL {
        if is_hidden(entry) {
            continue;
        }
        if previous_group.is_some_and(|group| group != entry.group()) {
            slots.push(EdenMenuSlot::Separator);
        }
        previous_group = Some(entry.group());
        slots.push(EdenMenuSlot::Entry(entry));
    }
    slots
}

thread_local! {
    /// [`visible_slots`] for an [`shell_context_menu::eden_entries_version`].
    static VISIBLE_SLOTS: RefCell<Option<(u64, Vec<EdenMenuSlot>)>> =
        const { RefCell::new(None) };
}

/// The visible Eden entries of the item menu (cheap per frame: recomputed
/// only after the user hid or showed an entry).
pub fn visible_slots() -> Vec<EdenMenuSlot> {
    let version = shell_context_menu::eden_entries_version();
    VISIBLE_SLOTS.with(|slot| {
        let mut slot = slot.borrow_mut();
        match &*slot {
            Some((cached, slots)) if *cached == version => slots.clone(),
            _ => {
                let hidden = shell_context_menu::hidden_eden_entries();
                let slots = layout(|entry| hidden.contains(entry.key()));
                *slot = Some((version, slots.clone()));
                slots
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn keys_are_unique_and_prefixed() {
        let keys: HashSet<&str> = EdenMenuEntry::ALL.iter().map(|e| e.key()).collect();
        assert_eq!(keys.len(), EdenMenuEntry::ALL.len());
        for entry in EdenMenuEntry::ALL {
            let key = entry.key();
            assert!(key.starts_with(EDEN_KEY_PREFIX), "{key}");
            assert!(key.len() > EDEN_KEY_PREFIX.len(), "{key}");
            assert!(!entry.label_key().is_empty());
        }
    }

    #[test]
    fn groups_are_contiguous() {
        // Separators are derived from group changes, so a group must not be
        // split by another one.
        let groups: Vec<u8> = EdenMenuEntry::ALL.iter().map(|e| e.group()).collect();
        assert!(groups.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn layout_collapses_separators() {
        use EdenMenuEntry::*;
        use EdenMenuSlot::{Entry, Separator};

        assert_eq!(layout(|_| false).len(), EdenMenuEntry::ALL.len() + 1);
        assert!(layout(|_| true).is_empty());

        // A whole group hidden: no separator at all.
        let first_group = [OpenInNewTab, OpenInSplitView, ManageTags, OpenWithDefault];
        let slots = layout(|e| first_group.contains(&e));
        assert!(!slots.contains(&Separator));
        assert_eq!(slots.first(), Some(&Entry(Cut)));

        let slots = layout(|e| e != OpenInNewTab && e != Properties);
        assert_eq!(
            slots,
            vec![Entry(OpenInNewTab), Separator, Entry(Properties)]
        );

        for slots in [layout(|e| e.group() == 1), layout(|e| e == Cut)] {
            assert_ne!(slots.first(), Some(&Separator));
            assert_ne!(slots.last(), Some(&Separator));
            assert!(
                !slots
                    .windows(2)
                    .any(|pair| pair[0] == Separator && pair[1] == Separator)
            );
        }
    }
}
