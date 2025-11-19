use std::{
    cell::UnsafeCell,
    fs::{File, OpenOptions},
    io::Write,
    ops::{Deref, DerefMut},
    path::PathBuf,
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct Locked<T> {
    readers: AtomicUsize,
    writer: AtomicBool,
    in_memory: UnsafeCell<T>,
    file: UnsafeCell<File>,
}

pub struct Key<'a, T: Serialize> {
    lock: &'a Locked<T>,
    data: NonNull<T>,
}

pub struct WritableKey<'a, T: Serialize> {
    lock: &'a Locked<T>,
}

impl<T: Serialize> Locked<T> {
    pub fn new(path: impl Into<PathBuf>, data: T) -> std::io::Result<Self> {
        let path_buf = path.into();
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .read(true)
            .open(&path_buf)?;
        
        // Write initial data
        if let Ok(json) = serde_json::to_string_pretty(&data) {
            use std::io::Write;
            (&file).write_all(json.as_bytes())?;
        }
        
        Ok(Self {
            readers: AtomicUsize::new(0),
            writer: AtomicBool::new(false),
            in_memory: UnsafeCell::new(data),
            file: UnsafeCell::new(file),
        })
    }

    pub fn load(path: impl Into<PathBuf>) -> std::io::Result<Self>
    where
        T: for<'de> Deserialize<'de>,
    {
        let file = OpenOptions::new()
            .write(true)
            .read(true)
            .open(path.into())?;

        let data: T = serde_json::from_reader(&file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        Ok(Self {
            readers: AtomicUsize::new(0),
            writer: AtomicBool::new(false),
            in_memory: UnsafeCell::new(data),
            file: UnsafeCell::new(file),
        })
    }

    pub fn read<'a>(&'a self) -> Key<'a, T> {
        while self.writer.load(Ordering::SeqCst) {
            std::hint::spin_loop();
        }

        self.readers.fetch_add(1, Ordering::SeqCst);
        Key {
            lock: &self,
            data: NonNull::new(self.in_memory.get()).unwrap(),
        }
    }

    pub fn write<'a>(&'a self) -> WritableKey<'a, T> {
        while self.writer.load(Ordering::SeqCst) || self.readers.load(Ordering::SeqCst) > 0 {
            std::hint::spin_loop();
        }

        self.writer.fetch_or(true, Ordering::SeqCst);
        WritableKey { lock: &self }
    }
}

impl<'a, T> Deref for Key<'a, T>
where
    T: Serialize,
{
    type Target = T;

    fn deref(&self) -> &'a Self::Target {
        unsafe { self.data.as_ref() }
    }
}

impl<'a, T> Deref for WritableKey<'a, T>
where
    T: Serialize,
{
    type Target = T;

    fn deref(&self) -> &'a Self::Target {
        unsafe { &*self.lock.in_memory.get() }
    }
}

impl<'a, T> DerefMut for WritableKey<'a, T>
where
    T: Serialize,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.in_memory.get() }
    }
}

impl<'a, T> Drop for Key<'a, T>
where
    T: Serialize,
{
    fn drop(&mut self) {
        self.lock.readers.fetch_sub(1, Ordering::SeqCst);
    }
}

impl<'a, T> Drop for WritableKey<'a, T>
where
    T: Serialize,
{
    fn drop(&mut self) {
        if let Ok(s) = serde_json::to_string_pretty(unsafe { &*self.lock.in_memory.get() }) {
            let _ = unsafe { (*self.lock.file.get()).write_all(s.as_bytes()) };
        }
        self.lock.writer.store(false, Ordering::SeqCst);
    }
}
