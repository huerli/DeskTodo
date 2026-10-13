//! 客户端认证加密：服务器只收到密文，256 位密钥保存在系统凭据库。
use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use uuid::Uuid;
use base64::{Engine,engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest,Sha256};
use serde::{Serialize,Deserialize};
use crate::model::ServerSettings;
use zeroize::Zeroizing;

const SERVICE: &str = "com.desktodo.app.server-encryption";
const IDENTITY_FILE: &str = ".server-key-id";
const MAGIC: &[u8; 4] = b"DTD1";
const HEADER_LEN: usize = 32;
static KEYRING_LOCK: Mutex<()> = Mutex::new(());

pub struct ServerKey {
    pub id: Uuid,
    key: Zeroizing<Vec<u8>>,
}

fn keyring_error(_: keyring::Error) -> String {
    // 不输出凭据 API 返回的原始错误，某些编码错误会附带秘密字节。
    "无法访问系统密钥管理器，请解锁系统凭据库后重试（不会降级为明文上传）".into()
}

fn get_key(entry: &keyring::Entry, create: bool) -> Result<Zeroizing<Vec<u8>>, String> {
    let key = match entry.get_secret() {
        Ok(key) => Zeroizing::new(key),
        Err(keyring::Error::NoEntry) if create => {
            let mut key = Zeroizing::new(vec![0; 32]);
            OsRng.try_fill_bytes(&mut key).map_err(|_| "系统安全随机数不可用")?;
            entry.set_secret(&key).map_err(keyring_error)?;
            let saved = Zeroizing::new(entry.get_secret().map_err(keyring_error)?);
            if *saved != *key {
                return Err("系统密钥管理器保存校验失败，已停止上传".into());
            }
            key
        }
        Err(keyring::Error::NoEntry) => {
            return Err("本设备的加密密钥已丢失，请恢复系统凭据库中的密钥。不会生成新密钥覆盖服务器数据".into());
        }
        Err(error) => return Err(keyring_error(error)),
    };
    if key.len() != 32 {
        return Err("系统密钥管理器中的加密密钥格式不正确，已停止上传".into());
    }
    Ok(key)
}

#[derive(Serialize,Deserialize)]
pub struct Pairing {
    pub version:u32,
    pub id:Uuid,
    key:Vec<u8>,
    pub server:ServerSettings,
    pub host_key:Option<String>,
}
impl Drop for Pairing {fn drop(&mut self) {use zeroize::Zeroize;self.key.zeroize();}}
impl Pairing {
    pub fn parse(code:&str)->Result<Self,String> {
        let mut parts=code.trim().split('.');
        if parts.next()!=Some("DT1") {return Err("配对码格式不正确".into());}
        let body=parts.next().ok_or("配对码不完整")?;
        let checksum=parts.next().ok_or("配对码不完整")?;
        if parts.next().is_some() || body.len()>32768 {return Err("配对码格式不正确".into());}
        let bytes=Zeroizing::new(URL_SAFE_NO_PAD.decode(body).map_err(|_|"配对码格式不正确")?);
        let digest=Sha256::digest(&bytes);
        if URL_SAFE_NO_PAD.encode(&digest[..8])!=checksum {return Err("配对码校验失败，请完整复制".into());}
        let pairing:Self=serde_json::from_slice(&bytes).map_err(|_|"配对码内容不正确")?;
        if pairing.version!=1 || pairing.key.len()!=32 {return Err("配对码版本或密钥长度不正确".into());}
        Ok(pairing)
    }
    pub fn install(&self,dir:&Path)->Result<(),String> {
        let _lock=KEYRING_LOCK.lock().map_err(|_|"系统密钥管理器访问锁异常")?;
        let entry=keyring::Entry::new(SERVICE,&self.id.to_string()).map_err(keyring_error)?;
        match entry.get_secret() {
            Ok(existing) => {let existing=Zeroizing::new(existing);if *existing!=self.key {return Err("此清单标识已存在但密钥不同，已拒绝覆盖".into());}}
            Err(keyring::Error::NoEntry)=>entry.set_secret(&self.key).map_err(keyring_error)?,
            Err(error)=>return Err(keyring_error(error)),
        }
        let saved=Zeroizing::new(entry.get_secret().map_err(keyring_error)?);
        if *saved!=self.key {return Err("共享密钥保存校验失败".into());}
        crate::model::atomic_write(&dir.join(IDENTITY_FILE),self.id.to_string().as_bytes())
    }
}

impl ServerKey {
    pub fn pairing_code(&self,mut server:ServerSettings,host_key:Option<String>)->Result<String,String> {
        server.ssh_key_path.clear();
        server.known_hosts_path.clear();
        server.last_upload=None;
        let pairing=Pairing {version:1,id:self.id,key:self.key.to_vec(),server,host_key};
        let bytes=Zeroizing::new(serde_json::to_vec(&pairing).map_err(|_|"生成配对码失败")?);
        let digest=Sha256::digest(&bytes);
        Ok(format!("DT1.{}.{}",URL_SAFE_NO_PAD.encode(&bytes),URL_SAFE_NO_PAD.encode(&digest[..8])))
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self { id: Uuid::new_v4(), key: Zeroizing::new(vec![7; 32]) }
    }

    /// 本地文件仅保存非秘密 UUID；解密密钥从不写入应用目录或配置。
    pub fn load(dir: &Path, allow_create: bool) -> Result<Self, String> {
        let _lock = KEYRING_LOCK.lock().map_err(|_| "系统密钥管理器访问锁异常")?;
        let identity = dir.join(IDENTITY_FILE);
        let (id, create) = match std::fs::read_to_string(&identity) {
            Ok(text) => (Uuid::parse_str(text.trim()).map_err(|_| "本设备加密标识损坏，已停止上传")?, false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_create => (Uuid::new_v4(), true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err("尚未创建本设备的加密密钥，请先上传或恢复原设备的加密标识与系统凭据".into());
            }
            Err(_) => return Err("无法读取本设备的加密标识，已停止上传".into()),
        };
        let entry = keyring::Entry::new(SERVICE, &id.to_string()).map_err(keyring_error)?;
        let key = get_key(&entry, create)?;
        if create {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&identity).map_err(|_| "无法保存加密标识，已停止上传")?;
            file.write_all(id.to_string().as_bytes()).map_err(|_| "保存加密标识失败，已停止上传")?;
            file.sync_all().map_err(|_| "保存加密标识失败，已停止上传")?;
        }
        Ok(Self { id, key })
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| "加密密钥格式不正确")?;
        let mut nonce_bytes = [0; 12];
        OsRng.try_fill_bytes(&mut nonce_bytes).map_err(|_| "系统安全随机数不可用，已停止上传")?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let mut output = Vec::with_capacity(HEADER_LEN + plaintext.len() + 16);
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(self.id.as_bytes());
        let ciphertext = cipher.encrypt(nonce, Payload { msg: plaintext, aad: &output })
            .map_err(|_| "数据加密失败，已停止上传")?;
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        Ok(output)
    }

    pub fn decrypt(&self, encrypted: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
        if encrypted.len() < HEADER_LEN + 16 || &encrypted[..4] != MAGIC {
            return Err("服务器数据格式不正确或版本不受支持".into());
        }
        if &encrypted[4..20] != self.id.as_bytes() {
            return Err("服务器数据属于其他设备，需要该设备的加密密钥".into());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|_| "加密密钥格式不正确")?;
        cipher.decrypt(Nonce::from_slice(&encrypted[20..32]), Payload {
            msg: &encrypted[32..], aad: &encrypted[..20],
        }).map(Zeroizing::new).map_err(|_| "服务器数据校验失败：密钥不匹配或密文被修改，本机待办未被覆盖".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key() -> ServerKey { ServerKey::for_test() }

    #[test]
    fn pairing_preserves_group_key_without_local_credentials() {
        let key=key();
        let mut server=ServerSettings::default();server.host="server.example".into();server.username="user".into();server.ssh_key_path="/private/key".into();
        let code=key.pairing_code(server,None).unwrap();
        let paired=Pairing::parse(&code).unwrap();
        assert_eq!(paired.id,key.id);assert_eq!(paired.key,*key.key);assert!(paired.server.ssh_key_path.is_empty());
        let peer=ServerKey {id:paired.id,key:Zeroizing::new(paired.key.clone())};
        assert_eq!(&*peer.decrypt(&key.encrypt(b"shared task").unwrap()).unwrap(),b"shared task");
        let mut damaged=code.into_bytes();let offset=10;damaged[offset]=if damaged[offset]==b'A' {b'B'}else{b'A'};
        assert!(Pairing::parse(&String::from_utf8(damaged).unwrap()).is_err());
    }

    #[test]
    fn encrypt_round_trip_uses_fresh_nonces() {
        let key = key();
        let plain = b"private task and credential";
        let first = key.encrypt(plain).unwrap();
        let second = key.encrypt(plain).unwrap();
        assert_ne!(first, second);
        assert!(!first.windows(plain.len()).any(|window| window == plain));
        assert_eq!(&*key.decrypt(&first).unwrap(), plain);
    }

    #[test]
    fn wrong_key_or_tampering_fails_closed() {
        let key = key();
        let ciphertext = key.encrypt(b"private task").unwrap();
        for index in [0, 4, 20, 32, ciphertext.len() - 1] {
            let mut damaged = ciphertext.clone();
            damaged[index] ^= 1;
            assert!(key.decrypt(&damaged).is_err());
        }
        let wrong = ServerKey { id: key.id, key: Zeroizing::new(vec![8; 32]) };
        assert!(wrong.decrypt(&ciphertext).is_err());
        assert!(key.decrypt(&[]).is_err());
    }

    #[test]
    fn missing_existing_key_is_never_recreated() {
        let entry = keyring::Entry::new_with_credential(Box::new(keyring::mock::MockCredential::default()));
        assert!(get_key(&entry, false).is_err());
        assert!(matches!(entry.get_secret(), Err(keyring::Error::NoEntry)));
        let generated = get_key(&entry, true).unwrap();
        assert_eq!(generated.len(), 32);
        assert_eq!(*get_key(&entry, false).unwrap(), *generated);
        entry.set_secret(&[1, 2]).unwrap();
        assert!(get_key(&entry, false).is_err());
    }
}
