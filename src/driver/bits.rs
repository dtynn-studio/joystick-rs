use std::ops::BitXor;

pub trait Bits: Sized + BitXor<Output = Self> + Default {
    const CAP: usize;

    fn bit(&self, pos: usize) -> Option<bool>;

    fn set(&mut self, pos: usize) -> bool;

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

            fn set(&mut self, pos: usize) -> bool {
                if pos >= Self::CAP {
                    return false;
                }

                *self |= (1 << pos);
                true
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
#[derive(Debug, Default)]
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

    fn set(&mut self, pos: usize) -> bool {
        if pos >= Self::CAP {
            return false;
        }

        if pos < 128 {
            self.0[0] |= 1 << pos;
        } else {
            self.0[1] |= 1 << (pos - 128);
        }

        true
    }

    fn count_ones(&self) -> u32 {
        self.0[0].count_ones() + self.0[1].count_ones()
    }
}
