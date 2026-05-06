use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, mpsc},
    time::Duration,
};

use parking_lot::Mutex;

use cdma_common::error::Error;
use log::{debug, trace};
use tokio::sync::Notify;
use types::{ChannelType, DataRequest, MacMessage};

pub mod types;

pub type Layer2MacRef = Arc<Layer2Mac>;

pub struct Layer2Mac {
    lac_to_mac_rx: Mutex<mpsc::Receiver<MacMessage>>,
    mac_to_lac_tx: mpsc::Sender<MacMessage>,
    fragments: Mutex<HashMap<ChannelType, VecDeque<DataRequest>>>,
    fragment_ready: Notify,
}

impl Layer2Mac {
    pub fn new(
        lac_to_mac_rx: mpsc::Receiver<MacMessage>,
        mac_to_lac_tx: mpsc::Sender<MacMessage>,
    ) -> Layer2MacRef {
        Arc::new(Layer2Mac {
            lac_to_mac_rx: Mutex::new(lac_to_mac_rx),
            mac_to_lac_tx,
            fragments: Mutex::new(HashMap::new()),
            fragment_ready: Notify::new(),
        })
    }

    pub fn send_mac_message(&self, msg: MacMessage) -> Result<(), Error> {
        self.mac_to_lac_tx.send(msg)?;
        Ok(())
    }

    pub fn get_fragment(&self, channel_type: ChannelType) -> Result<Option<DataRequest>, Error> {
        if let Some(entry) = self.fragments.lock().get_mut(&channel_type) {
            return Ok(entry.pop_front());
        }
        Ok(None)
    }

    /// Waits until a fragment for `channel_type` is available, then returns it.
    pub async fn wait_for_fragment(&self, channel_type: ChannelType) -> Result<DataRequest, Error> {
        loop {
            if let Some(entry) = self.fragments.lock().get_mut(&channel_type) {
                if let Some(req) = entry.pop_front() {
                    return Ok(req);
                }
            }
            self.fragment_ready.notified().await;
        }
    }

    pub fn start(&self) -> Result<(), Error> {
        let rx = self.lac_to_mac_rx.lock();
        loop {
            let msg = rx.recv()?;
            trace!("LAC -> MAC RX: {:?}", msg);
            self.handle_lac_message(msg);
        }
    }

    pub fn run_for(&self, max_messages: usize, recv_timeout: Duration) -> Result<usize, Error> {
        debug!(
            "Starting bounded MAC listener: max_messages={}, recv_timeout={:?}",
            max_messages, recv_timeout
        );
        let rx = self.lac_to_mac_rx.lock();
        let mut handled = 0usize;
        while handled < max_messages {
            match rx.recv_timeout(recv_timeout) {
                Ok(msg) => {
                    trace!("LAC -> MAC RX: {:?}", msg);
                    self.handle_lac_message(msg);
                    handled += 1;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(handled)
    }

    fn handle_lac_message(&self, msg: MacMessage) {
        match msg {
            MacMessage::DataRequest(data_request) => {
                let mut fragments = self.fragments.lock();
                let queue = fragments.entry(data_request.channel_type).or_default();
                queue.push_back(data_request);
                drop(fragments);
                self.fragment_ready.notify_one();
            }
            _ => {
                debug!("unsupported mac message");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use cdma_common::{bits::Bitstream, lac::message_types::MessageId, time::cdma_epoch};

    use crate::{
        lac::MessageControlStatusBlock,
        mac::{
            Layer2Mac,
            types::{AvailabilityIndication, ChannelType, DataRequest, MacMessage},
        },
    };

    fn mcsb(
        channel: ChannelType,
        message_id: MessageId,
        length_bits: usize,
    ) -> MessageControlStatusBlock {
        MessageControlStatusBlock {
            channel,
            mobile_p_rev: None,
            extended_encryption: false,
            message_id,
            length_bits,
            requested_tx_time: None,
            tx_deadline: None,
            address: None,
            ack_seq: 0,
            msg_seq: 0,
            ack_req: false,
            valid_ack: false,
            overhead_mcc: 0x03ff,
            overhead_imsi_11_12: 0x7f,
        }
    }

    fn data_request(channel_type: ChannelType, bits: &[u8]) -> DataRequest {
        DataRequest {
            channel_type,
            size: bits.len(),
            data: Bitstream::new_init(bits),
            mcsb: mcsb(channel_type, MessageId::Order, bits.len()),
        }
    }

    #[test]
    fn data_requests_queue_fifo_per_channel() {
        let (lac_to_mac_tx, lac_to_mac_rx) = mpsc::channel();
        let (mac_to_lac_tx, _mac_to_lac_rx) = mpsc::channel();
        let mac = Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

        lac_to_mac_tx
            .send(MacMessage::DataRequest(data_request(
                ChannelType::FPch,
                &[1, 0, 1],
            )))
            .unwrap();
        lac_to_mac_tx
            .send(MacMessage::DataRequest(data_request(
                ChannelType::FTch,
                &[0, 1, 0, 1],
            )))
            .unwrap();
        lac_to_mac_tx
            .send(MacMessage::DataRequest(data_request(
                ChannelType::FPch,
                &[1, 1, 0],
            )))
            .unwrap();

        assert_eq!(mac.run_for(3, Duration::from_millis(10)).unwrap(), 3);

        let first_pch = mac.get_fragment(ChannelType::FPch).unwrap().unwrap();
        assert_eq!(first_pch.data.bits(), &[1, 0, 1]);
        assert_eq!(first_pch.size, 3);
        assert_eq!(first_pch.mcsb.channel, ChannelType::FPch);

        let first_tch = mac.get_fragment(ChannelType::FTch).unwrap().unwrap();
        assert_eq!(first_tch.data.bits(), &[0, 1, 0, 1]);
        assert_eq!(first_tch.size, 4);
        assert_eq!(first_tch.mcsb.channel, ChannelType::FTch);

        let second_pch = mac.get_fragment(ChannelType::FPch).unwrap().unwrap();
        assert_eq!(second_pch.data.bits(), &[1, 1, 0]);

        assert!(mac.get_fragment(ChannelType::FPch).unwrap().is_none());
        assert!(mac.get_fragment(ChannelType::FTch).unwrap().is_none());
    }

    #[test]
    fn availability_indication_forwards_to_lac() {
        let (_lac_to_mac_tx, lac_to_mac_rx) = mpsc::channel();
        let (mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let mac = Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

        mac.send_mac_message(MacMessage::AvailabilityIndication(AvailabilityIndication {
            channel_type: ChannelType::FPch,
            max_size: 170,
            system_time: cdma_epoch(),
            sync_superframe_start: true,
            chip_cursor: 12_288,
        }))
        .unwrap();

        match mac_to_lac_rx
            .recv_timeout(Duration::from_millis(10))
            .unwrap()
        {
            MacMessage::AvailabilityIndication(indication) => {
                assert_eq!(indication.channel_type, ChannelType::FPch);
                assert_eq!(indication.max_size, 170);
                assert_eq!(indication.system_time, cdma_epoch());
                assert!(indication.sync_superframe_start);
                assert_eq!(indication.chip_cursor, 12_288);
            }
            other => panic!("expected AvailabilityIndication, got {other:?}"),
        }
    }
}
