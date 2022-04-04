#[macro_use]
extern crate enum_display_derive;

mod core;
mod device;
mod status;

use array_tool::vec::Intersect;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

use crate::core::{CoreType, CoreStatus, Core};
use crate::device::DeviceType;

lazy_static! {
    static ref REGEX_DEVICE_CORE: Regex = Regex::new(r"^(npu)(?P<idx>\d+)pe.*$").unwrap();
}

pub async fn list_async() -> Vec<Core> {
    list_async_with_path("/dev", "/sys").await
}

async fn list_async_with_path(devfs: &str, sysfs: &str) -> Vec<Core> {
    let device_map = build_device_map_async(devfs).await
        .unwrap_or_else(|e| {
            eprintln!("WARN: Failed getting device list {}", e);
            HashMap::new()
        });

    let mut res: Vec<Core> = vec![];
    for (idx, paths) in device_map.into_iter() {
        if is_furiosa_device(idx, sysfs).await {
            if let Some(device_type) = identify_device_type(idx, sysfs).await {
                let mut cores = collect_cores(idx, device_type, paths).await;
                res.append(&mut cores);
            }
        }
    }
    res.sort();

    res
}

async fn collect_cores(idx: u8, device_type: DeviceType, paths: Vec<PathBuf>) -> Vec<Core> {
    let mut res: Vec<Core> = vec![];
    for path in paths.into_iter() {
        if let Some(core) = get_core(idx, path, device_type).await {
            res.push(core);
        }
    }

    core_status_masking(res)
}

fn core_status_masking(cores: Vec<Core>) -> Vec<Core> {
    let occupied: Vec<u8> = cores.iter()
        .filter(|core| core.status() == CoreStatus::Occupied)
        .flat_map(|core| match core.core_type() {
            CoreType::Single(idx) => vec![*idx],
            CoreType::Fusion(v) => v.clone()
        })
        .collect();

    cores.into_iter()
        .map(|core| {
            let is_occupied = core.status() == CoreStatus::Available &&
                match core.core_type() {
                    CoreType::Single(idx) =>
                        occupied.contains(idx),
                    CoreType::Fusion(indexes) =>
                        occupied.intersect(indexes.clone()).len() > 0
                };

            if is_occupied {
                core.with_status(CoreStatus::Occupied2)
            } else {
                core
            }
        })
        .collect()
}

async fn get_core(device_idx: u8, core_path: PathBuf, device_type: DeviceType) -> Option<Core> {
    let status = status::get_core_status(&core_path).await;
    let file_name = core_path.file_name().unwrap().to_string_lossy().to_string();

    if let Ok(core_type) = CoreType::try_from(file_name.as_str()) {
        Some(Core::new(
            device_idx,
            core_path,
            core_type,
            device_type,
            status))
    } else {
        None
    }
}

async fn build_device_map_async(devfs: &str) -> tokio::io::Result<HashMap<u8, Vec<PathBuf>>> {
    let mut dir = fs::read_dir(devfs).await?;

    let mut map: HashMap<u8, Vec<PathBuf>> = HashMap::new();
    while let Some(entry) = dir.next_entry().await? {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(x) = REGEX_DEVICE_CORE.captures(&name) {
            let idx: u8 = x.name("idx").unwrap()
                .as_str()
                .parse()
                .unwrap();

            map.entry(idx).or_insert_with(Vec::new).push(entry.path());
        }
    }

    Ok(map)
}

async fn is_furiosa_device(idx: u8, sysfs: &str) -> bool {
    let path = format!("{}/class/npu_mgmt/npu{}_mgmt/platform_type", sysfs, idx);

    fs::read_to_string(path).await
        .ok()
        .filter(|s| s.trim() == "FuriosaAI")
        .is_some()
}

async fn identify_device_type(idx: u8, sysfs: &str) -> Option<DeviceType> {
    let path = format!("{}/class/npu_mgmt/npu{}_mgmt/device_type", sysfs, idx);

    let text = fs::read_to_string(path).await;
    if let Ok(device_type) = text {
        DeviceType::try_from(device_type.as_str()).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_build_device_map_async() -> tokio::io::Result<()> {
        let res = build_device_map_async("tests/test-0/dev").await?;
        //assert_eq!(res, vec![0, 1]);
        println!("{:?}", res);
        Ok(())
    }

    #[tokio::test]
    async fn test_is_furiosa_device() -> tokio::io::Result<()> {
        let res = is_furiosa_device(0, "tests/test-0/sys").await;
        assert!(res);

        let res = is_furiosa_device(1, "tests/test-0/sys").await;
        assert!(res);

        let res = is_furiosa_device(2, "tests/test-0/sys").await;
        assert!(!res);

        Ok(())
    }

    #[tokio::test]
    async fn test_identify_device() -> tokio::io::Result<()> {
        let res = identify_device_type(0, "tests/test-0/sys").await;
        println!("{:?}", res);

        let res = identify_device_type(1, "tests/test-0/sys").await;
        println!("{:?}", res);

        let res = identify_device_type(2, "tests/test-0/sys").await;
        println!("{:?}", res);

        Ok(())
    }

    #[tokio::test]
    async fn test_get_core() -> tokio::io::Result<()> {
        let buf = PathBuf::from("tests/test-0/dev/npu0pe0");
        let res = get_core(0, buf, DeviceType::Warboy).await.unwrap();
        println!("{:?}", res);
        assert_eq!("npu0:0", res.name());
        assert_eq!("tests/test-0/dev/npu0pe0", res.path().as_os_str().to_string_lossy().as_ref());
        assert_eq!(1, res.core_count());
        assert!(!res.is_fusioned());

        Ok(())
    }

    #[tokio::test]
    async fn test_core_status_masking() -> tokio::io::Result<()> {
        let cores = vec![
            Core::new(0, PathBuf::new(),
                CoreType::Single(0),
                DeviceType::Warboy,
                CoreStatus::Available)
        ];

        let res = core_status_masking(cores/*, occupied*/);
        assert_eq!(res.len(), 1);
        let core0 = res.get(0).unwrap();
        assert_eq!(core0.core_type(), &CoreType::Single(0));
        assert_eq!(core0.status(), CoreStatus::Available);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Warboy,
                      CoreStatus::Occupied)
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Occupied]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Warboy,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Warboy,
                      CoreStatus::Available),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Available, CoreStatus::Available]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Warboy,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Warboy,
                      CoreStatus::Occupied),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Available, CoreStatus::Occupied]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Warboy,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Warboy,
                      CoreStatus::Occupied),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1]),
                      DeviceType::Warboy,
                      CoreStatus::Available),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Available, CoreStatus::Occupied, CoreStatus::Occupied2]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Warboy,
                      CoreStatus::Occupied),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Warboy,
                      CoreStatus::Occupied),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1]),
                      DeviceType::Warboy,
                      CoreStatus::Available),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Occupied, CoreStatus::Occupied, CoreStatus::Occupied2]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Warboy,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Warboy,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1]),
                      DeviceType::Warboy,
                      CoreStatus::Occupied),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Occupied2, CoreStatus::Occupied2, CoreStatus::Occupied]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(2),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(3),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1, 2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Available, CoreStatus::Available, CoreStatus::Available,
                             CoreStatus::Available, CoreStatus::Available, CoreStatus::Available, CoreStatus::Available,]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Renegade,
                      CoreStatus::Occupied),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(2),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(3),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1, 2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Occupied, CoreStatus::Available, CoreStatus::Available,
                             CoreStatus::Available, CoreStatus::Occupied2, CoreStatus::Available, CoreStatus::Occupied2,]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(2),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(3),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Occupied),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1, 2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Available, CoreStatus::Available, CoreStatus::Occupied2, CoreStatus::Occupied2,
                             CoreStatus::Available, CoreStatus::Occupied, CoreStatus::Occupied2,]);

        let cores = vec![
            Core::new(0, PathBuf::new(),
                      CoreType::Single(0),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(1),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(2),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Single(3),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Available),
            Core::new(0, PathBuf::new(),
                      CoreType::Fusion(vec![0, 1, 2, 3]),
                      DeviceType::Renegade,
                      CoreStatus::Occupied),
        ];

        let res = core_status_masking(cores/*, occupied*/).into_iter()
            .map(|c| c.status())
            .collect::<Vec<CoreStatus>>();
        assert_eq!(res, vec![CoreStatus::Occupied2, CoreStatus::Occupied2, CoreStatus::Occupied2, CoreStatus::Occupied2,
                             CoreStatus::Occupied2, CoreStatus::Occupied2, CoreStatus::Occupied,]);

        Ok(())
    }

}