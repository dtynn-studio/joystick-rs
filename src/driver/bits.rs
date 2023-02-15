use std::ops::BitXor;

pub trait Bits: Sized + BitXor<Output = Self> {
    const CAP: usize;

    fn bit(&self, pos: usize) -> Option<bool>;

    fn count_ones(&self) -> u32;
}

macro_rules! impl_bits {
    ($t:ty, $cap:literal) => {
        impl Bits for $t {
            const CAP: usize = $cap;

            #[inline]
            fn bit(&self, pos: usize) -> Option<bool> {
                if pos >= Self::CAP {
                    return None;
                }

                Some((self & (1 << pos)) != 0)
            }

            #[inline]
            fn count_ones(&self) -> u32 {
                <$t>::count_ones(*self)
            }
        }
    };
}

impl_bits!(u32, 32);
impl_bits!(u64, 64);
impl_bits!(u128, 128);

#[repr(transparent)]
pub struct B256([u128; 2]);

impl BitXor for B256 {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        B256([self.0[0] ^ rhs.0[0], self.0[1] ^ rhs.0[1]])
    }
}

impl Bits for B256 {
    const CAP: usize = 256;

    fn bit(&self, pos: usize) -> Option<bool> {
        if pos >= Self::CAP {
            return None;
        }

        Some(if pos < 128 {
            (self.0[0] & (1 << pos)) != 0
        } else {
            (self.0[1] & (1 << (pos - 128))) != 0
        })
    }

    fn count_ones(&self) -> u32 {
        self.0[0].count_ones() + self.0[1].count_ones()
    }
}
