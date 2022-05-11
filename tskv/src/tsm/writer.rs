use std::{
    collections::HashMap,
    io::{Seek, SeekFrom, Write},
};

use super::{block, MAX_BLOCK_VALUES};
use crate::{
    error::{Error, Result},
    DataBlock, FileBlock, FileCursor,
};

// A TSM file is composed for four sections: header, blocks, index and the footer.
//
// ┌────────┬────────────────────────────────────┬─────────────┬──────────────┐
// │ Header │               Blocks               │    Index    │    Footer    │
// │5 bytes │              N bytes               │   N bytes   │   4 bytes    │
// └────────┴────────────────────────────────────┴─────────────┴──────────────┘
//
// ┌───────────────────┐
// │      Header       │
// ├─────────┬─────────┤
// │  Magic  │ Version │
// │ 4 bytes │ 1 byte  │
// └─────────┴─────────┘
//
// ┌───────────────────────────────────────┐
// │               Blocks                  │
// ├───────────────────┬───────────────────┤
// │                Block                  │
// ├─────────┬─────────┼─────────┬─────────┼
// │  CRC    │ ts      │  CRC    │  value  │
// │ 4 bytes │ N bytes │ 4 bytes │ N bytes │
// └─────────┴─────────┴─────────┴─────────┴
//
//  ──────────────────────────────────────────────────────────────────┐
// │                                   Index                          │
// ┬─────────┬──────┬───────┬─────────┬─────────┬────────┬────────┬───┤
// │ filedId │ Type │ Count │Min Time │Max Time │ Offset │  Size  │...│
// │ 8 bytes │1 byte│2 bytes│ 8 bytes │ 8 bytes │8 bytes │8 bytes │   │
// ┴─────────┴──────┴───────┴─────────┴─────────┴────────┴────────┴───┘
//
// ┌─────────┐
// │ Footer  │
// ├─────────┤
// │Index Ofs│
// │ 8 bytes │
// └─────────┘

const HEADER_LEN: u64 = 5;
const TSM_MAGIC: u32 = 0x1346613;
const VERSION: u8 = 1;

pub struct FooterBuilder {
    writer: FileCursor,
}
impl FooterBuilder {
    pub fn new(writer: FileCursor) -> Self {
        Self { writer }
    }
    pub fn build(&mut self, offset: u64) -> Result<()> {
        self.writer
            .write(&mut offset.to_be_bytes().to_vec())
            .map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
        Ok(())
    }
}
pub struct TsmIndexBuilder {
    writer: FileCursor,
}

impl TsmIndexBuilder {
    pub fn new(writer: FileCursor) -> Self {
        Self { writer }
    }

    pub fn build(&mut self, indexs: HashMap<u64, Vec<FileBlock>>) -> Result<u64> {
        let res = self.writer.pos();
        for (fid, blks) in indexs {
            let mut buf = Vec::new();
            let block = blks.first().unwrap();
            // let typ:u8 = block.filed_type.into();
            let typ: u8 = 1;
            let cnt: u16 = blks.len() as u16;
            buf.append(&mut fid.to_be_bytes().to_vec());
            buf.append(&mut typ.to_be_bytes().to_vec());
            buf.append(&mut cnt.to_be_bytes().to_vec());
            // build index block
            for blk in blks {
                buf.append(&mut blk.min_ts.to_be_bytes().to_vec());
                buf.append(&mut blk.max_ts.to_be_bytes().to_vec());
                buf.append(&mut blk.offset.to_be_bytes().to_vec());
                buf.append(&mut blk.size.to_be_bytes().to_vec());
            }
            self.writer.write(&buf).map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
        }
        Ok(res)
    }
}

pub struct TsmBlockWriter {
    writer: FileCursor,
}

impl TsmBlockWriter {
    pub fn new(writer: FileCursor) -> Self {
        Self { writer }
    }
}

impl TsmBlockWriter {
    fn build(&mut self, mut block: DataBlock) -> Result<Vec<FileBlock>> {
        let filed_type = block.filed_type();
        let len = block.len();
        let n = (len - 1) / MAX_BLOCK_VALUES + 1;
        let mut res = Vec::with_capacity(n);
        let mut i = 0;
        let mut last_index = 0;
        while i < n {
            let start = last_index;
            let end = len % MAX_BLOCK_VALUES + i * MAX_BLOCK_VALUES;
            last_index = end;
            let (min_ts, max_ts) = block.time_range(start, end);
            let (ts_buf, data_buf) = block.encode(start, end)?;
            if self.writer.pos() <= HEADER_LEN {
                let mut buf = Vec::with_capacity(HEADER_LEN as usize);
                buf.append(&mut TSM_MAGIC.to_be_bytes().to_vec());
                buf.append(&mut VERSION.to_be_bytes().to_vec());
                self.writer
                    .seek(SeekFrom::Start(0))
                    .map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
                self.writer
                    .write(&buf)
                    .map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
            }
            // fill data if err occur reset the pos
            let offset = self.writer.pos();
            self.writer
                .write(&mut crc32fast::hash(&ts_buf).to_be_bytes().to_vec())
                .map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
            self.writer
                .write(&ts_buf)
                .map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
            self.writer
                .write(&mut crc32fast::hash(&data_buf).to_be_bytes().to_vec())
                .map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
            self.writer
                .write(&data_buf)
                .map_err(|e| Error::WriteTsmErr { reason: e.to_string() })?;
            let size = ts_buf.len() + data_buf.len();
            res.push(FileBlock { min_ts,
                                 max_ts,
                                 offset,
                                 filed_type,
                                 size: size as u64,
                                 reader_idx: 0 });
            i += 1;
        }
        Ok(res)
    }
}

#[cfg(test)]
mod test {
    use crate::{tsm::coders, DataBlock, StrCell};

    #[test]
    fn test_str_encode() {
        // let block = DataBlock::new(10, crate::DataType::Str(StrCell{ts:1, val: vec![]}));
        // block.insert(crate::DataType::Str(StrCell{ts:1, val: vec![1]}));
        // block.insert(crate::DataType::Str(StrCell{ts:2, val: vec![2]}));
        // block.insert(crate::DataType::Str(StrCell{ts:3, val: vec![3]}));
        let mut data = vec![];
        let str = vec![vec![1_u8]];
        let tmp: Vec<&[u8]> = str.iter().map(|x| &x[..]).collect();
        coders::string::encode(&tmp, &mut data);
    }
}
