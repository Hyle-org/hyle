pub trait Pick<T> {
    fn get(&self) -> &T;
    fn get_mut(&mut self) -> &mut T;
    /// # Safety
    /// This function is unsafe because it returns a raw pointer to the inner value.
    /// This is intended for splitting borrows.
    unsafe fn splitting_get_mut(&mut self) -> *mut T;
}

macro_rules! generic_tuple {
    // base case
    (
        $pub:vis struct $name:ident ($t1:ty$(,$t2:ty)*$(,)?);
    ) => {
        #[allow(unused_imports)]
        use paste::paste;
        generic_tuple! {
            aa: $pub struct $name ( $($t2,)* ) {
                a: $t1,
            }
        }
        impl $crate::utils::generic_tuple::Pick<$t1> for $name {
            fn get(&self) -> &$t1 {
                &self.a
            }
            fn get_mut(&mut self) -> &mut $t1 {
                &mut self.a
            }
            unsafe fn splitting_get_mut(&mut self) -> *mut $t1 {
                &mut self.a as *mut $t1
            }
        }
    };
    // general case
    (
        $index:ident: $pub:vis struct $name:ident ($t1:ty, $($t2:ty,)*) { $($idx:ident: $t3:ty,)+ }
    ) => {
        paste! {
            generic_tuple! {
                [<a $index>]: $pub struct $name ( $($t2,)* ) {
                    $($idx: $t3,)*
                    $index: $t1,
                }
            }
        }
        impl $crate::utils::generic_tuple::Pick<$t1> for $name {
            fn get(&self) -> &$t1 {
                &self.$index
            }
            fn get_mut(&mut self) -> &mut $t1 {
                &mut self.$index
            }
            unsafe fn splitting_get_mut(&mut self) -> *mut $t1 {
                &mut self.$index as *mut $t1
            }
        }
    };
    // end case
    (
        $index:ident: $pub:vis struct $name:ident ( ) { $($idx:ident: $t3:ty,)+ }
    ) => {
        $pub struct $name {
            $($idx: $t3,)+
        }
        impl $name {
            pub fn new($($idx: $t3,)*) -> Self {
                Self {
                    $($idx,)*
                }
            }
        }
    };
}
pub(crate) use generic_tuple;
