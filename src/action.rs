//! Every user-triggerable action: names are `[keys]` config names;
//! descriptions feed help, footer and the command palette.

use crate::define_actions;

define_actions! {
    pub enum Action {
        // global
        Quit => "quit", "Quit", ["q", "ctrl-c"],
        Help => "help", "Show keys & help", ["?", "f1"],
        Palette => "palette", "Command palette", [":", "ctrl-p"],
        NextTab => "next_tab", "Next tab", ["tab", "L"],
        PrevTab => "prev_tab", "Previous tab", ["shift-tab", "H"],
        Tab1 => "tab_playback", "Go to Playback", ["1"],
        Tab2 => "tab_recording", "Go to Recording", ["2"],
        Tab3 => "tab_outputs", "Go to Outputs", ["3"],
        Tab4 => "tab_inputs", "Go to Inputs", ["4"],
        Tab5 => "tab_cards", "Go to Cards", ["5"],
        Tab6 => "tab_virtual", "Go to Virtual devices", ["6"],
        Tab7 => "tab_settings", "Go to Settings", ["7"],
        Tab8 => "tab_privacy", "Go to Privacy", ["8"],
        Up => "up", "Move up", ["k", "up"],
        Down => "down", "Move down", ["j", "down"],
        Top => "top", "Jump to top", ["g", "home"],
        Bottom => "bottom", "Jump to bottom", ["G", "end"],
        PageUp => "page_up", "Page up", ["pgup", "ctrl-u"],
        PageDown => "page_down", "Page down", ["pgdn", "ctrl-d"],
        Filter => "filter", "Filter list", ["/"],
        Menu => "menu", "Actions for selection", ["."],
        Details => "details", "Show/hide details panel", ["i"],
        Apply => "apply", "Apply system changes (write drop-ins, restart)", ["ctrl-s"],
        // mixer (all four mixer tabs)
        VolUp => "volume_up", "Volume up", ["l", "right", "+", "="],
        VolDown => "volume_down", "Volume down", ["h", "left", "-"],
        VolUpFine => "volume_up_fine", "Volume up 1%", ["shift-right", ">"],
        VolDownFine => "volume_down_fine", "Volume down 1%", ["shift-left", "<"],
        SetVolume => "set_volume", "Type an exact volume", ["v"],
        Mute => "mute", "Mute / unmute", ["m"],
        Channels => "channels", "Per-channel volume (expand)", ["c"],
        Alias => "alias", "Display name (this app only)", ["a"],
        // streams
        MoveTo => "move_to", "Send to another device", ["enter", "o"],
        FollowDefault => "follow_default", "Follow the default device", ["f"],
        AlwaysHere => "always_here", "Always send this app to its current device", ["R"],
        Remember => "remember", "Always remember this app, even with app memory off", ["s"],
        // devices
        SetDefault => "set_default", "Make default", ["enter", "d"],
        Port => "port", "Choose port (speakers/headphones/HDMI…)", ["p"],
        Profile => "profile", "Choose card profile / Bluetooth codec", ["P"],
        Rename => "rename", "Rename everywhere (system)", ["r"],
        Disable => "disable", "Disable device (system)", ["D"],
        Latency => "latency", "Latency offset", ["t"],
        // cards
        CardProfile => "card_profile", "Choose profile / codec", ["enter", "p"],
        // virtual
        NewVirtual => "new_virtual", "New virtual device", ["n"],
        DeleteVirtual => "delete_virtual", "Remove virtual device", ["x", "delete"],
        TempSink => "temp_sink", "Quick temporary virtual output", ["t"],
        // settings / privacy rows
        Change => "change", "Change selected setting", ["enter", "space"],
        Reset => "reset", "Reset to default", ["x"],
        Forget => "forget", "Forget selected remembered entry", ["x", "delete"],
        ForgetFile => "forget_file", "Forget everything in this file", ["X"],
        Reload => "reload", "Re-read WirePlumber's state files", ["F5"],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctx {
    Streams,
    Devices,
    Cards,
    Virtual,
    Settings,
    Privacy,
}

impl Action {
    pub fn available(self, ctx: Ctx) -> bool {
        use Action::*;
        use Ctx::*;
        match self {
            VolUp | VolDown | VolUpFine | VolDownFine | SetVolume | Mute | Channels | Alias => matches!(ctx, Streams | Devices),
            MoveTo | FollowDefault | AlwaysHere | Remember => ctx == Streams,
            SetDefault | Port | Profile | Latency => ctx == Devices,
            Rename | Disable => matches!(ctx, Devices | Cards),
            CardProfile => ctx == Cards,
            NewVirtual | DeleteVirtual | TempSink => ctx == Virtual,
            Change => matches!(ctx, Settings | Privacy),
            Reset => ctx == Settings,
            Forget | ForgetFile | Reload => ctx == Privacy,
            Filter => matches!(ctx, Streams | Devices),
            _ => true,
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(self, Action::Mute | Action::SetDefault | Action::Change)
    }

    pub fn footer(ctx: Ctx) -> &'static [Action] {
        use Action::*;
        match ctx {
            Ctx::Streams => &[VolDown, VolUp, Mute, MoveTo, Channels, Menu, Help],
            Ctx::Devices => &[VolDown, VolUp, Mute, SetDefault, Port, Profile, Rename, Menu, Help],
            Ctx::Cards => &[CardProfile, Rename, Disable, Help],
            Ctx::Virtual => &[NewVirtual, DeleteVirtual, TempSink, Apply, Help],
            Ctx::Settings => &[Change, Reset, Apply, Help],
            Ctx::Privacy => &[Change, Forget, ForgetFile, Apply, Help],
        }
    }

    pub fn section(self) -> &'static str {
        use Action::*;
        match self {
            VolUp | VolDown | VolUpFine | VolDownFine | SetVolume | Mute | Channels | Alias => "Mixer tabs (1–4)",
            MoveTo | FollowDefault | AlwaysHere | Remember => "Playback & Recording",
            SetDefault | Port | Profile | Latency | Rename | Disable => "Outputs, Inputs & Cards",
            CardProfile => "Outputs, Inputs & Cards",
            NewVirtual | DeleteVirtual | TempSink => "Virtual devices",
            Change | Reset | Forget | ForgetFile | Reload => "Settings & Privacy",
            _ => "General",
        }
    }
}
