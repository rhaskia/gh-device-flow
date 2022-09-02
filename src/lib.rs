
use std::{result::Result, thread, time, fmt};
use std::collections::HashMap;

use chrono::{DateTime, Duration};
use chrono::offset::Utc;

mod util;

#[derive(Debug, Default, Clone, serde_derive::Serialize)]
pub struct Credential {
    pub token: String,
    pub expiry: String,
    pub refresh_token: String,
}

impl Credential {
    fn empty() -> Credential {
        Credential {
            token: String::new(),
            expiry: String::new(),
            refresh_token: String::new(),
        }
    }

    pub fn is_expired(&self) -> bool {
        let exp = match DateTime::parse_from_rfc3339(self.expiry.as_str()) {
            Ok(time) => time,
            Err(_) => return false
        };
        let now = Utc::now();
        now > exp
    }
}


#[derive(Debug, Clone)]
pub enum DeviceFlowError {
  HttpError(String),
  GitHubError(String),
}

impl fmt::Display for DeviceFlowError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            DeviceFlowError::HttpError(string) => write!(f, "DeviceFlowError: {}", string),
            DeviceFlowError::GitHubError(string) => write!(f, "DeviceFlowError: {}", string)
        }
    }
}

impl std::error::Error for DeviceFlowError {}

impl From<reqwest::Error> for DeviceFlowError {
    fn from(e: reqwest::Error) -> Self {
        DeviceFlowError::HttpError(format!("{:?}", e))
    }
}

pub fn authorize(client_id: String, host: Option<String>) -> Result<Credential, DeviceFlowError> {
    let my_string: String;
    let thost = match host {
        Some(string) => {
            my_string = string;
            Some(my_string.as_str())
        },
        None => None
    };

    let mut flow = DeviceFlow::start(client_id.as_str(), thost)?;

    // eprintln!("res is {:?}", res);
    eprintln!("Please visit {} in your browser", flow.verification_uri.clone().unwrap());
    eprintln!("And enter code: {}", flow.user_code.clone().unwrap());

    thread::sleep(FIVE_SECONDS);

    flow.poll()
}

pub fn refresh(client_id: &str, refresh_token: &str, host: Option<String>) -> Result<Credential, DeviceFlowError> {
    let my_string: String;
    let thost = match host {
        Some(string) => {
            my_string = string;
            Some(my_string.as_str())
        },
        None => None
    };

    refresh_access_token(client_id, refresh_token, thost)
}

#[derive(Debug, Clone)]
pub enum DeviceFlowState {
    Pending,
    Processing(time::Duration),
    Success(Credential),
    Failure(DeviceFlowError)
}

#[derive(Clone)]
pub struct DeviceFlow {
    pub host: String,
    pub client_id: String,
    pub user_code: Option<String>,
    pub device_code: Option<String>,
    pub verification_uri: Option<String>,
    pub state: DeviceFlowState,
}

const FIVE_SECONDS: time::Duration = time::Duration::new(5, 0);

impl DeviceFlow {
    pub fn new(client_id: &str, maybe_host: Option<&str>) -> Self {
        Self{
            client_id: String::from(client_id),
            host: match maybe_host {
                Some(string) => String::from(string),
                None => String::from("github.com")
            },
            user_code: None,
            device_code: None,
            verification_uri: None,
            state: DeviceFlowState::Pending,
        }
    }

    pub fn start(client_id: &str, maybe_host: Option<&str>) -> Result<DeviceFlow, DeviceFlowError> {
        let mut flow = DeviceFlow::new(client_id, maybe_host);

        flow.setup();

        match flow.state {
            DeviceFlowState::Processing(_) => Ok(flow.to_owned()),
            DeviceFlowState::Failure(err) => Err(err),
            _ => Err(util::credential_error("Something truly unexpected happened".into()))
        }
    }

    pub fn setup(&mut self) {
        let body = format!("client_id={}", &self.client_id);
        let entry_url = format!("https://{}/login/device/code", &self.host);

        let res_opt = util::send_request(self, entry_url, body);

        match res_opt {
            Some(res) => {
                if res.contains_key("error") && res.contains_key("error_description"){
                    self.state = DeviceFlowState::Failure(util::credential_error(res["error_description"].as_str().unwrap().into()))
                } else if res.contains_key("error") {
                     self.state = DeviceFlowState::Failure(util::credential_error(format!("Error response: {:?}", res["error"].as_str().unwrap())))
                } else {
                    self.user_code = Some(String::from(res["user_code"].as_str().unwrap()));
                    self.device_code = Some(String::from(res["device_code"].as_str().unwrap()));
                    self.verification_uri = Some(String::from(res["verification_uri"].as_str().unwrap()));
                    self.state = DeviceFlowState::Processing(FIVE_SECONDS);
                }
            },
            None => {}
        }
    }

    pub fn poll(&mut self) -> Result<Credential, DeviceFlowError> {
        let mut count = 0u32;
        let mut credential: Option<Credential> = None;
        let client = reqwest::blocking::Client::new();
        let poll_url = format!("https://{}/login/oauth/access_token", self.host);
        let poll_payload = format!("client_id={}&device_code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code",
            self.client_id,
            &self.device_code.clone().unwrap()
        );

        loop {
            count += 1;
            let res = client.post(&poll_url)
                .header("Accept", "application/json")
                .body(poll_payload.clone())
                .send()?
                .json::<HashMap<String, serde_json::Value>>()?;

            if res.contains_key("error") {
                match res["error"].as_str().unwrap() {
                    "authorization_pending" => {},
                    "slow_down" => thread::sleep(FIVE_SECONDS),
                    other_reason => {
                        return Err(util::credential_error(format!("Error checking for token: {}", other_reason)))
                    },
                }
            } else {
                let mut this_credential = Credential::empty();
                this_credential.token = res["access_token"].as_str().unwrap().to_string();

                match res.get("expires_in") {
                    Some(expires_in) => {
                        this_credential.expiry = calculate_expiry(expires_in.as_i64().unwrap());
                        this_credential.refresh_token = res["refresh_token"].as_str().unwrap().to_string();
                    },
                    None => {}
                }
                credential = Some(this_credential);

                break
            }

            if count > 20 {
                break
            }
            thread::sleep(FIVE_SECONDS);
        };

        match credential {
            Some(cred) => Ok(cred),
            None => Err(util::credential_error("Unable to fetch credential, sorry :/".into()))
        }
    }
}

fn calculate_expiry(expires_in: i64) -> String {
    let expires_in = Duration::seconds(expires_in);
    let mut expiry: DateTime<Utc> = Utc::now();
    expiry = expiry + expires_in;
    expiry.to_rfc3339()
}

fn refresh_access_token(client_id: &str, refresh_token: &str, maybe_host: Option<&str>) -> Result<Credential, DeviceFlowError> {
    let host = match maybe_host {
        Some(string) => string,
        None => "github.com"
    };

    let client = reqwest::blocking::Client::new();
    let entry_url = format!("https://{}/login/oauth/access_token", &host);
    let request_body = format!("client_id={}&refresh_token={}&client_secret=&grant_type=refresh_token",
        &client_id, &refresh_token);

    let res = client.post(&entry_url)
        .header("Accept", "application/json")
        .body(request_body)
        .send()?
        .json::<HashMap<String, serde_json::Value>>()?;

    if res.contains_key("error") {
        Err(util::credential_error(res["error"].as_str().unwrap().into()))
    } else {
        let mut credential = Credential::empty();
        // eprintln!("res: {:?}", &res);
        credential.token = res["access_token"].as_str().unwrap().to_string();

        match res.get("expires_in") {
            Some(expires_in) => {
                credential.expiry = calculate_expiry(expires_in.as_i64().unwrap());
                credential.refresh_token = res["refresh_token"].as_str().unwrap().to_string();
            },
            None => {}
        }
        Ok(credential.clone())
    }
}
