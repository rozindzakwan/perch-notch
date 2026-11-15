//! Spike: can an unpackaged Win32 process read other apps' notifications
//! through `UserNotificationListener`? Prints the access status and, if
//! allowed, the current toasts (app name and text). Read-only.

#[cfg(windows)]
fn main() {
    use windows::UI::Notifications::Management::UserNotificationListener;
    use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds};
    let listener = match UserNotificationListener::Current() {
        Ok(l) => l,
        Err(e) => {
            println!("Current() failed: {e}");
            return;
        }
    };
    println!("GetAccessStatus: {:?}", listener.GetAccessStatus());
    match listener.RequestAccessAsync().and_then(|op| op.get()) {
        Ok(s) => println!("RequestAccessAsync: {s:?}"),
        Err(e) => println!("RequestAccessAsync failed: {e}"),
    }
    match listener
        .GetNotificationsAsync(NotificationKinds::Toast)
        .and_then(|op| op.get())
    {
        Ok(list) => {
            println!("{} toasts in the action center", list.Size().unwrap_or(0));
            for n in list {
                let app = n
                    .AppInfo()
                    .and_then(|a| a.DisplayInfo())
                    .and_then(|d| d.DisplayName())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let texts: Vec<String> = n
                    .Notification()
                    .and_then(|t| t.Visual())
                    .and_then(|v| v.GetBinding(&KnownNotificationBindings::ToastGeneric()?))
                    .and_then(|b| b.GetTextElements())
                    .map(|els| {
                        els.into_iter()
                            .filter_map(|e| e.Text().ok())
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                // Only the app name and the number of text lines: the spike
                // must not print anyone's message content.
                println!("- {app}: {} text line(s)", texts.len());
            }
        }
        Err(e) => println!("GetNotificationsAsync failed: {e}"),
    }
    match listener.NotificationChanged(&windows::Foundation::TypedEventHandler::new(|_, _| Ok(())))
    {
        Ok(_) => println!("NotificationChanged event: subscribed"),
        Err(e) => println!("NotificationChanged event failed: {e}"),
    }
}

#[cfg(not(windows))]
fn main() {}
