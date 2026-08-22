use crate::yandex::is_yandex_music_url;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use teloxide::types::UserId;
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Service {
    Yandex,
    TikTok,
    YouTube,
    Instagram,
    Other,
}

impl Service {
    pub fn all() -> &'static [Service] {
        &[
            Service::Yandex,
            Service::TikTok,
            Service::YouTube,
            Service::Instagram,
            Service::Other,
        ]
    }

    pub fn from_url(url: &str) -> Self {
        if is_yandex_music_url(url) {
            Service::Yandex
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
            Service::TikTok => "TikTok",
            Service::YouTube => "YouTube",
            Service::Instagram => "Instagram",
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
        if let Ok(mut set) = self.active_users.try_lock() {
            set.remove(&self.user_id);
        }
    }
}

pub struct ServiceGuard {
    _domain_guard: OwnedMutexGuard<()>,
    _global_guard: OwnedSemaphorePermit,
    _user_guard: UserGuard,
    waiters: Arc<AtomicUsize>,
}

impl Drop for ServiceGuard {
    fn drop(&mut self) {
        self.waiters.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct QueueManager {
    global_semaphore: Arc<Semaphore>,
    domain_locks: Arc<HashMap<Service, Arc<Mutex<()>>>>,
    domain_waiters: Arc<HashMap<Service, Arc<AtomicUsize>>>,
    active_users: Arc<Mutex<HashSet<UserId>>>,
}

impl QueueManager {
    pub fn new(max_global: usize) -> Self {
        let mut locks = HashMap::new();
        let mut waiters = HashMap::new();

        for &s in Service::all() {
            locks.insert(s, Arc::new(Mutex::new(())));
            waiters.insert(s, Arc::new(AtomicUsize::new(0)));
        }

        Self {
            global_semaphore: Arc::new(Semaphore::new(max_global)),
            domain_locks: Arc::new(locks),
            domain_waiters: Arc::new(waiters),
            active_users: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Tries to lock the user for a single active task. Returns false if user already has an active download.
    pub async fn try_lock_user(&self, user_id: UserId) -> bool {
        let mut users = self.active_users.lock().await;
        users.insert(user_id)
    }

    pub fn unlock_user(&self, user_id: UserId) {
        if let Ok(mut users) = self.active_users.try_lock() {
            users.remove(&user_id);
        }
    }

    pub fn register(&self, url: &str) -> (Service, usize) {
        let service = Service::from_url(url);
        let waiters = self.domain_waiters.get(&service).unwrap();
        let pos = waiters.fetch_add(1, Ordering::SeqCst) + 1;
        (service, pos)
    }

    pub async fn acquire(&self, service: Service, user_id: UserId) -> ServiceGuard {
        let lock_arc = self.domain_locks.get(&service).unwrap().clone();
        let waiters_arc = self.domain_waiters.get(&service).unwrap().clone();

        let domain_guard = lock_arc.lock_owned().await;
        let global_guard = self.global_semaphore.clone().acquire_owned().await.unwrap();

        ServiceGuard {
            _domain_guard: domain_guard,
            _global_guard: global_guard,
            _user_guard: UserGuard {
                user_id,
                active_users: self.active_users.clone(),
            },
            waiters: waiters_arc,
        }
    }
}
