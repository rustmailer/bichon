//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use mail_parser::{parsers::MessageStream, MessageParser, MimeHeaders};

use crate::{
    base64_encode_url_safe,
    modules::{account::entity::Encryption, imap::client::Client},
};

#[tokio::test]
async fn testxx() {
    rustls::crypto::CryptoProvider::install_default(rustls::crypto::ring::default_provider())
        .unwrap();
    let client = Client::connection("imap.zoho.com".into(), &Encryption::Ssl, 993, None, false)
        .await
        .unwrap();
    let mut session = client.login("xx@zohomail.com", "xxx").await.unwrap();
    session.select("INBOX").await.unwrap();
    let result = session.uid_search("LARGER 1024").await.unwrap();
    println!("{:#?}", result);
}

#[tokio::test]
async fn test1() {
    let path = r"C:\Users\polly\Downloads\test.eml";
    let eml_data = std::fs::read(path).unwrap();
    let input = base64_encode_url_safe!(eml_data);
    let message = MessageParser::default().parse(&input).unwrap();
    let parts = message.parts;
    for part in parts {
        println!("{}", part.is_message());
        println!("{}", part.is_multipart());
    }
}

#[tokio::test]
async fn test2() {
    const MESSAGE: &str = r#"From: Art Vandelay <art@vandelay.com> (Vandelay Industries)
To: "Colleagues": "James Smythe" <james@vandelay.com>; Friends:
    jane@example.com, =?UTF-8?Q?John_Sm=C3=AEth?= <john@example.com>;
Date: Sat, 20 Nov 2021 14:22:01 -0800
Subject: =?utf-8?B?SnVzdCAxNSBkYXlzIGxlZnQgdG8gdmlzaXQgTkFSTklBISDinYTvuI/wn462?=
Content-Type: multipart/mixed; boundary="festivus";

--festivus
Content-Type: text/html; charset="us-ascii"
Content-Transfer-Encoding: base64

PGh0bWw+PHA+SSB3YXMgdGhpbmtpbmcgYWJvdXQgcXVpdHRpbmcgdGhlICZsZHF1bztle
HBvcnRpbmcmcmRxdW87IHRvIGZvY3VzIGp1c3Qgb24gdGhlICZsZHF1bztpbXBvcnRpbm
cmcmRxdW87LDwvcD48cD5idXQgdGhlbiBJIHRob3VnaHQsIHdoeSBub3QgZG8gYm90aD8
gJiN4MjYzQTs8L3A+PC9odG1sPg==
--festivus
Content-Type: message/rfc822

From: "Cosmo Kramer" <kramer@kramerica.com>
Subject: Exporting my book about coffee tables
Content-Type: multipart/mixed; boundary="giddyup";

--giddyup
Content-Type: text/plain; charset="utf-16"
Content-Transfer-Encoding: quoted-printable

=FF=FE=0C!5=D8"=DD5=D8)=DD5=D8-=DD =005=D8*=DD5=D8"=DD =005=D8"=
=DD5=D85=DD5=D8-=DD5=D8,=DD5=D8/=DD5=D81=DD =005=D8*=DD5=D86=DD =
=005=D8=1F=DD5=D8,=DD5=D8,=DD5=D8(=DD =005=D8-=DD5=D8)=DD5=D8"=
=DD5=D8=1E=DD5=D80=DD5=D8"=DD!=00
--giddyup
Content-Type: image/gif; name*1="about "; name*0="Book ";
              name*2*=utf-8''%e2%98%95 tables.gif
Content-Transfer-Encoding: Base64
Content-Disposition: attachment

R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7
--giddyup--
--festivus--
"#;

    let message = MessageParser::default().parse(MESSAGE).unwrap();
    let raw_subject = message.header_raw("Subject").unwrap().as_bytes();

    let data = MessageStream::new(raw_subject)
        .parse_unstructured()
        .unwrap_text()
        .to_string();

    println!("{}", data);
    // RFC2047 support for encoded text in message readers
    println!("{}", message.subject().unwrap());
}

#[tokio::test]
async fn test44() {
    let path = r"C:\Users\polly\Downloads\test333.eml";
    let input = std::fs::read(path).unwrap();
    let message = MessageParser::default().parse(&input).unwrap();
    for attachment in message.attachments() {
        let content_type = attachment.content_type().unwrap();
        let filename = attachment
            .attachment_name()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let disposition = attachment.content_disposition();

        let file_type = format!(
            "{}/{}",
            content_type.c_type.as_ref(),
            content_type.c_subtype.as_deref().unwrap_or("")
        );

        let inline = disposition.map(|d| d.is_inline()).unwrap_or(false);

        println!(
            "filename: {}, file_type: {}, inline: {}",
            filename, file_type, inline
        );
    }
}
