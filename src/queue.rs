use crate::providers::pornhub::is_pornhub_url;
use crate::providers::soundcloud::is_soundcloud_url;
use crate::providers::yandex::is_yandex_music_url;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use teloxide::types::UserId;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Service {
    Yandex,
    SoundCloud,
    TikTok,
    YouTube,
    Instagram,
    Pornhub,
    Other,
}

impl Service {
    pub fn all() -> &'static [Service] {
        &[
            Service::Yandex,
            Service::SoundCloud,
            Service::TikTok,
            Service::YouTube,
            Service::Instagram,
            Service::Pornhub,
            Service::Other,
        ]
    }

    pub fn from_url(url: &str) -> Self {
        if is_yandex_music_url(url) {
            Service::Yandex
        } else if is_soundcloud_url(url) {
            Service::SoundCloud
        } else if is_pornhub_url(url) {
            Service::Pornhub
        } else if url.contains("tiktok.com") {
            Service::TikTok
        } else if url.contains("youtube.com") || url.contains("youtu.be") {
            Service::YouTube
        } else if url.contains("instagram.com") || url.contains("instagr.am") {
            Service::Instagram
        } else {
            Service::Other
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Service::Yandex => "Яндекс Музыке",
            Service::SoundCloud => "SoundCloud",
            Service::TikTok => "TikTok",
            Service::YouTube => "YouTube",
            Service::Instagram => "Instagram",
            Service::Pornhub => "Pornhub",
            Service::Other => "сторонним ресурсам",
        }
    }
}

pub struct UserGuard {
    user_id: UserId,
    active_users: Arc<Mutex<HashSet<UserId>>>,
}

impl Drop for UserGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.active_users.lock() {
            set.remove(&self.user_id);
        }
    }
}

pub struct WaiterGuard {
    waiters: Arc<AtomicUsize>,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        self.waiters.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct ServiceGuard {
    _domain_guard: OwnedSemaphorePermit,
    _global_guard: OwnedSemaphorePermit,
    _waiter_guard: WaiterGuard,
}

#[derive(Clone)]
pub struct QueueManager {
    global_semaphore: Arc<Semaphore>,
    domain_semaphores: Arc<HashMap<Service, Arc<Semaphore>>>,
    domain_concurrency: usize,
    domain_waiters: Arc<HashMap<Service, Arc<AtomicUsize>>>,
    active_users: Arc<Mutex<HashSet<UserId>>>,
}

impl QueueManager {
    pub fn new(max_global: usize) -> Self {
        let max_global = max_global.max(1);
        let domain_concurrency = max_global.clamp(1, 2);
        let mut semaphores = HashMap::new();
        let mut waiters = HashMap::new();

        for &s in Service::all() {
            semaphores.insert(s, Arc::new(Semaphore::new(domain_concurrency)));
            waiters.insert(s, Arc::new(AtomicUsize::new(0)));
        }

        Self {
            global_semaphore: Arc::new(Semaphore::new(max_global)),
            domain_semaphores: Arc::new(semaphores),
            domain_concurrency,
            domain_waiters: Arc::new(waiters),
            active_users: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Tries to acquire a download lock for a user.
    /// Returns `Some(UserGuard)` if allowed, or `None` if the user already has an active task.
    pub fn try_lock_user(&self, user_id: UserId) -> Option<UserGuard> {
        let mut users = self.active_users.lock().ok()?;
        if users.insert(user_id) {
            Some(UserGuard {
                user_id,
                active_users: Arc::clone(&self.active_users),
            })
        } else {
            None
        }
    }

    /// Registers the request in the service queue and returns the service type, queue position,
    /// and a `WaiterGuard` that automatically decrements the waiter counter on drop.
    pub fn register(&self, url: &str) -> (Service, usize, WaiterGuard) {
        let service = Service::from_url(url);
        let waiters = Arc::clone(self.domain_waiters.get(&service).unwrap());
        let active_and_waiting = waiters.fetch_add(1, Ordering::SeqCst) + 1;
        let pos = if active_and_waiting <= self.domain_concurrency {
            1
        } else {
            active_and_waiting - self.domain_concurrency + 1
        };
        (service, pos, WaiterGuard { waiters })
    }

    /// Waits for both a per-domain slot and a global download slot, returning RAII `ServiceGuard`.
    pub async fn acquire(&self, service: Service, waiter_guard: WaiterGuard) -> ServiceGuard {
        let domain_sem = Arc::clone(self.domain_semaphores.get(&service).unwrap());
        let domain_guard = domain_sem
            .acquire_owned()
            .await
            .expect("domain semaphore closed unexpectedly");
        let global_guard = Arc::clone(&self.global_semaphore)
            .acquire_owned()
            .await
            .expect("global semaphore closed unexpectedly");

        ServiceGuard {
            _domain_guard: domain_guard,
            _global_guard: global_guard,
            _waiter_guard: waiter_guard,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_from_url() {
        assert_eq!(
            Service::from_url("https://soundcloud.com/artist/track"),
            Service::SoundCloud
        );
        assert_eq!(
            Service::from_url("https://music.yandex.ru/track/123"),
            Service::Yandex
        );
        assert_eq!(
            Service::from_url("https://www.youtube.com/watch?v=123"),
            Service::YouTube
        );
        assert_eq!(
            Service::from_url("https://www.pornhub.com/view_video.php?viewkey=123"),
            Service::Pornhub
        );
    }

    #[tokio::test]
    async fn test_parallel_domain_slots() {
        let q = QueueManager::new(3);
        let (s1, pos1, w1) = q.register("https://youtube.com/watch?v=1");
        let (s2, pos2, w2) = q.register("https://youtube.com/watch?v=2");
        let (_, pos3, _w3) = q.register("https://youtube.com/watch?v=3");

        // First two requests on the same domain can run concurrently (pos == 1), third queues (pos == 2)
        assert_eq!(pos1, 1);
        assert_eq!(pos2, 1);
        assert_eq!(pos3, 2);

        let _g1 = q.acquire(s1, w1).await;
        let _g2 = q.acquire(s2, w2).await;
    }
}
