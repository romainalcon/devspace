//! DNS answers for the local domains.
//!
//! The daemon runs a minimal authoritative DNS server on `127.0.0.1`. The
//! operating system sends it the queries for `*.<tld>` only (see the
//! resolver installed by `devspace system setup`); it answers every name
//! of that zone with the loopback address of the sites:
//!
//! - `A` → the address of the sites (`127.0.0.2` by default);
//! - other record types, including `AAAA` (the sites are served over IPv4
//!   only) → no data (`NOERROR` without answers);
//! - names outside the zone → `REFUSED` (the server never recurses);
//! - malformed queries → `FORMERR`, or no answer at all when not even the
//!   header can be read.
//!
//! Only standard queries with a single question are accepted (RFC 1035);
//! the additional section (e.g. an EDNS `OPT` record) is ignored, and
//! responses are always far below the 512-byte UDP limit.

use std::net::Ipv4Addr;

use devspace_helper::Tld;

/// Time-to-live of the answers, in seconds.
pub const TTL_SECS: u32 = 60;

const HEADER_LEN: usize = 12;
const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;

const RCODE_NOERROR: u8 = 0;
const RCODE_FORMERR: u8 = 1;
const RCODE_NOTIMP: u8 = 4;
const RCODE_REFUSED: u8 = 5;

/// A parsed question.
struct Question<'a> {
    /// Lowercase name labels.
    labels: Vec<String>,
    qtype: u16,
    qclass: u16,
    /// Raw question bytes (name, type and class), echoed in the response.
    raw: &'a [u8],
}

/// Computes the response to the DNS `query` for the zone `zone` whose
/// names all resolve to `address`, or `None` when the query must be
/// ignored (not a query, or too short to answer).
#[must_use]
pub fn answer(query: &[u8], zone: &Tld, address: Ipv4Addr) -> Option<Vec<u8>> {
    if query.len() < HEADER_LEN {
        return None;
    }
    let flags = query[2];
    let is_response = flags & 0x80 != 0;
    if is_response {
        return None;
    }
    let opcode = (flags >> 3) & 0x0f;
    let recursion_desired = flags & 0x01;
    let question_count = u16::from_be_bytes([query[4], query[5]]);
    if opcode != 0 {
        return Some(header_only(query, recursion_desired, RCODE_NOTIMP));
    }
    if question_count != 1 {
        return Some(header_only(query, recursion_desired, RCODE_FORMERR));
    }
    let Some(question) = parse_question(&query[HEADER_LEN..]) else {
        return Some(header_only(query, recursion_desired, RCODE_FORMERR));
    };

    let in_zone = question
        .labels
        .last()
        .is_some_and(|tld| tld == zone.as_str());
    if !in_zone || question.qclass != CLASS_IN {
        return Some(response(
            query,
            recursion_desired,
            RCODE_REFUSED,
            &question,
            None,
        ));
    }
    let octets = address.octets();
    let data = (question.qtype == TYPE_A).then_some(&octets[..]);
    Some(response(
        query,
        recursion_desired,
        RCODE_NOERROR,
        &question,
        data,
    ))
}

/// Parses the question at the start of `section`. Compression pointers are
/// refused: they make no sense in the first question.
fn parse_question(section: &[u8]) -> Option<Question<'_>> {
    let mut labels = Vec::new();
    let mut offset = 0;
    let mut name_len = 0usize;
    loop {
        let len = usize::from(*section.get(offset)?);
        offset += 1;
        if len == 0 {
            break;
        }
        // Label types other than plain labels (pointers, extended).
        if len > 63 {
            return None;
        }
        let label = section.get(offset..offset + len)?;
        name_len += len + 1;
        if name_len > 255 {
            return None;
        }
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        offset += len;
    }
    let fixed = section.get(offset..offset + 4)?;
    let qtype = u16::from_be_bytes([fixed[0], fixed[1]]);
    let qclass = u16::from_be_bytes([fixed[2], fixed[3]]);
    Some(Question {
        labels,
        qtype,
        qclass,
        raw: &section[..offset + 4],
    })
}

/// Response header: same id, QR and AA set, RD copied, given rcode and
/// counts.
fn header(query: &[u8], recursion_desired: u8, rcode: u8, questions: u16, answers: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&query[0..2]);
    out.push(0x80 | 0x04 | recursion_desired);
    out.push(rcode);
    out.extend_from_slice(&questions.to_be_bytes());
    out.extend_from_slice(&answers.to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

fn header_only(query: &[u8], recursion_desired: u8, rcode: u8) -> Vec<u8> {
    header(query, recursion_desired, rcode, 0, 0)
}

fn response(
    query: &[u8],
    recursion_desired: u8,
    rcode: u8,
    question: &Question<'_>,
    data: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = header(
        query,
        recursion_desired,
        rcode,
        1,
        u16::from(data.is_some()),
    );
    out.extend_from_slice(question.raw);
    if let Some(data) = data {
        // Name: pointer to the question name, right after the header.
        out.extend_from_slice(&[0xc0, 0x0c]);
        out.extend_from_slice(&question.qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out.extend_from_slice(&TTL_SECS.to_be_bytes());
        // At most 16 bytes.
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITES: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);
    const TYPE_AAAA: u16 = 28;

    fn answer(query: &[u8]) -> Option<Vec<u8>> {
        super::answer(query, &Tld::default(), SITES)
    }

    fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&[0x01, 0x00]); // RD
        out.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
        for label in name.split('.') {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        out
    }

    fn rcode(response: &[u8]) -> u8 {
        response[3] & 0x0f
    }

    fn answer_count(response: &[u8]) -> u16 {
        u16::from_be_bytes([response[6], response[7]])
    }

    /// Data of the single answer, which follows the echoed question:
    /// name pointer (2), type (2), class (2), TTL (4), length (2), data.
    fn rdata<'a>(response: &'a [u8], query: &[u8]) -> &'a [u8] {
        let record = &response[query.len()..];
        assert_eq!(&record[..2], &[0xc0, 0x0c], "name points to the question");
        assert_eq!(&record[6..10], &TTL_SECS.to_be_bytes());
        let len = usize::from(u16::from_be_bytes([record[10], record[11]]));
        assert_eq!(record.len(), 12 + len);
        &record[12..]
    }

    #[test]
    fn local_names_resolve_to_loopback() {
        let q = query(0x1234, "blog.internal", TYPE_A);
        let r = answer(&q).unwrap();
        assert_eq!(&r[0..2], &[0x12, 0x34]);
        assert_eq!(r[2] & 0x80, 0x80, "QR");
        assert_eq!(r[2] & 0x04, 0x04, "AA");
        assert_eq!(r[2] & 0x01, 0x01, "RD copied");
        assert_eq!(rcode(&r), RCODE_NOERROR);
        assert_eq!(answer_count(&r), 1);
        assert_eq!(&r[HEADER_LEN..q.len()], &q[HEADER_LEN..], "question echoed");
        assert_eq!(rdata(&r, &q), &[127, 0, 0, 2]);

        let q = query(1, "API.Sub.Blog.INTERNAL", TYPE_A);
        let r = answer(&q).unwrap();
        assert_eq!(rcode(&r), RCODE_NOERROR);
        assert_eq!(rdata(&r, &q), &[127, 0, 0, 2]);

        let r = super::answer(&q, &"test".parse().unwrap(), Ipv4Addr::LOCALHOST).unwrap();
        assert_eq!(rcode(&r), RCODE_REFUSED, "another zone is not served");
    }

    #[test]
    fn other_types_have_no_data() {
        for qtype in [15, TYPE_AAAA] {
            let r = answer(&query(1, "blog.internal", qtype)).unwrap();
            assert_eq!(rcode(&r), RCODE_NOERROR);
            assert_eq!(answer_count(&r), 0);
        }
    }

    #[test]
    fn names_outside_the_zone_are_refused() {
        for name in ["example.com", "blog.test", "internal.com", "blog.internals"] {
            let r = answer(&query(1, name, TYPE_A)).unwrap();
            assert_eq!(rcode(&r), RCODE_REFUSED, "{name}");
            assert_eq!(answer_count(&r), 0);
        }
    }

    #[test]
    fn malformed_queries_are_rejected() {
        assert!(answer(&[0; 5]).is_none());
        let mut response = query(1, "blog.internal", TYPE_A);
        response[2] |= 0x80;
        assert!(answer(&response).is_none(), "responses are ignored");

        let mut truncated = query(1, "blog.internal", TYPE_A);
        truncated.truncate(truncated.len() - 3);
        assert_eq!(rcode(&answer(&truncated).unwrap()), RCODE_FORMERR);

        let mut two_questions = query(1, "blog.internal", TYPE_A);
        two_questions[5] = 2;
        assert_eq!(rcode(&answer(&two_questions).unwrap()), RCODE_FORMERR);

        let mut pointer = query(1, "blog.internal", TYPE_A);
        pointer[HEADER_LEN] = 0xc0;
        assert_eq!(rcode(&answer(&pointer).unwrap()), RCODE_FORMERR);

        let mut notify = query(1, "blog.internal", TYPE_A);
        notify[2] |= 4 << 3;
        assert_eq!(rcode(&answer(&notify).unwrap()), RCODE_NOTIMP);
    }

    #[test]
    fn every_input_is_handled_without_panicking() {
        let base = query(7, "a.blog.internal", TYPE_A);
        for len in 0..=base.len() {
            let _ = answer(&base[..len]);
        }
        for byte in 0..=255u8 {
            let mut q = base.clone();
            q[HEADER_LEN] = byte;
            let _ = answer(&q);
        }
    }
}
